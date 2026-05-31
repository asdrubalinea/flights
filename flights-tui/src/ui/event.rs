//! Input: poll crossterm for one frame's worth of time and translate a key press
//! into a UI [`Action`]. crossterm comes from `ratatui::crossterm` (ratatui 0.30
//! re-exports its matching version — there is no separate crossterm dependency).

use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::app::Mode;

/// A user intent, decoupled from the specific key that produced it.
pub enum Action {
    Quit,
    SelectNext,
    SelectPrev,
    ClearSelection,
    /// Open the flight-detail popup on the selected (or nearest) flight.
    OpenDetail,
    /// Close the popup, returning to the radar.
    CloseDetail,
    /// Scroll the popup body up/down by one page.
    ScrollUp,
    ScrollDown,
    None,
}

/// Wait up to `timeout` for a key press and map it to an [`Action`]. The same key
/// can mean different things per [`Mode`] — `Enter` opens the popup on the radar,
/// `Esc` clears the selection on the radar but closes the popup in detail mode.
/// Returns [`Action::None`] on timeout or any key we don't bind, so the caller
/// still redraws (keeping dead-reckoned blips gliding).
pub fn next_action(timeout: Duration, mode: Mode) -> std::io::Result<Action> {
    if !event::poll(timeout)? {
        return Ok(Action::None);
    }
    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };
    // Ignore key-release/repeat events (crossterm reports them on some platforms).
    if key.kind != KeyEventKind::Press {
        return Ok(Action::None);
    }

    // Quit and selection stepping mean the same thing in both modes.
    let action = match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => Action::Quit,
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            Action::Quit
        }
        KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
        KeyCode::Up | KeyCode::Char('k') => Action::SelectPrev,
        // Mode-dependent keys.
        KeyCode::Enter if mode == Mode::Radar => Action::OpenDetail,
        KeyCode::Esc => match mode {
            Mode::Detail => Action::CloseDetail,
            Mode::Radar => Action::ClearSelection,
        },
        KeyCode::PageUp if mode == Mode::Detail => Action::ScrollUp,
        KeyCode::PageDown if mode == Mode::Detail => Action::ScrollDown,
        _ => Action::None,
    };
    Ok(action)
}
