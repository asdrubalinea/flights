//! The radar-style TUI, a thin Client (ADR-0005): a Home-centered scope of blips
//! beside a list of the same flights and a status line. Every frame it re-queries
//! the Server's `/picture` — which costs no Source call, since the Server
//! dead-reckons on read — and renders the response. It holds no engine; the only
//! state is what the Server last told it.
//!
//! Split per the implementation plan: [`app`] holds state and the logic behind
//! each input, [`render`] draws, [`event`] turns key presses into
//! [`event::Action`]s.

mod app;
mod event;
mod render;

pub use app::App;

use event::Action;

/// Run the TUI: install the terminal, then loop — poll `/picture`, draw, handle
/// input — until the user quits. The terminal is restored on exit and, via the
/// panic hook `ratatui::init` installs, on panic too.
pub fn run(mut app: App) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    while app.running {
        // Re-query the Server each frame; loopback and free of any Source call.
        app.refresh();

        terminal.draw(|frame| render::draw(frame, app))?;

        // Block for input only up to one frame; the timeout paces the redraw.
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

        // If the action opened the popup or moved the selection within it, fetch
        // the new flight's detail now so the next frame paints it immediately.
        app.sync_detail();
    }
    Ok(())
}
