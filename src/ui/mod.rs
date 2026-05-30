//! The radar-style TUI: a Home-centered scope of dead-reckoned blips beside a
//! list of the same flights and a status line. The screen refreshes at the
//! configured frame rate — kept current by dead reckoning, costing no API calls —
//! while the poller streams Snapshots in over a channel on its own cadence.
//!
//! Split per the implementation plan: [`app`] holds state and input logic,
//! [`render`] draws, [`event`] turns key presses into [`event::Action`]s.

mod app;
mod event;
mod render;

use std::sync::mpsc::{self, TryRecvError};

use crate::config::Config;
use crate::poller;
use crate::sources;

use app::App;
use event::Action;

/// Run the TUI: build the Source, spawn the poller, then loop drawing frames and
/// handling input until the user quits. The terminal is restored on exit and,
/// via the panic hook `ratatui::init` installs, on panic too.
pub fn run(cfg: &Config) -> anyhow::Result<()> {
    let source = sources::build(cfg)?;
    let area = cfg.search_area();
    let bounds = crate::poll_bounds(cfg, &*source);
    let tracker_cfg = crate::tracker_cfg(cfg);
    let source_name = source.name().to_string();

    let (tx, rx) = mpsc::channel();
    let (_poller, shutdown) = poller::spawn(source, area, bounds, tracker_cfg, tx);

    let mut app = App::new(
        area,
        tracker_cfg,
        source_name,
        cfg.render_interval(),
        cfg.search.relevance_distance_nm,
    );

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();

    // Signal the poller to stop, but deliberately don't *join* it: a fetch may be
    // parked in a (read-only) HTTP call for up to the request timeout, and the
    // user shouldn't have to wait for that just to quit. The poller holds nothing
    // that needs flushing, so letting process exit reap the thread is safe.
    drop(shutdown);

    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mpsc::Receiver<poller::PollUpdate>,
) -> anyhow::Result<()> {
    while app.running {
        // Drain everything the poller has sent since the last frame.
        loop {
            match rx.try_recv() {
                Ok(update) => app.on_update(update),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.note_poller_stopped();
                    break;
                }
            }
        }

        terminal.draw(|frame| render::draw(frame, app))?;

        // Block for input only up to one frame; the timeout is what paces the
        // dead-reckoned redraw.
        match event::next_action(app.fps_interval, app.mode)? {
            Action::Quit => app.running = false,
            Action::SelectNext => app.select_next(),
            Action::SelectPrev => app.select_prev(),
            Action::ClearSelection => app.clear_selection(),
            Action::OpenDetail => app.open_detail(),
            Action::CloseDetail => app.close_detail(),
            Action::ScrollUp => app.scroll_detail(-5),
            Action::ScrollDown => app.scroll_detail(5),
            Action::None => {}
        }
    }
    Ok(())
}
