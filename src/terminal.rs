use std::{
    io,
    sync::{Arc, Mutex},
};

use crossterm::{
    cursor::{self, MoveTo, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{DefaultTerminal, Terminal, backend::CrosstermBackend};

type State = Arc<Mutex<Option<(u16, u16)>>>;

pub struct TerminalGuard {
    state: State,
}

impl TerminalGuard {
    pub fn new() -> Self {
        let state = State::default();
        let panic_state = state.clone();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(&panic_state);
            hook(info);
        }));
        Self { state }
    }

    pub fn enter(&mut self) -> io::Result<DefaultTerminal> {
        let origin = cursor::position()?;
        terminal::enable_raw_mode()?;
        *self.state.lock().unwrap_or_else(|error| error.into_inner()) = Some(origin);
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Terminal::new(CrosstermBackend::new(io::stdout()))
    }

    pub fn leave(&self) {
        restore(&self.state);
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

fn restore(state: &State) {
    let origin = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some((x, y)) = origin {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let (x, y) = terminal::size().map_or((x, y), |(width, height)| {
            (
                x.min(width.saturating_sub(1)),
                y.min(height.saturating_sub(1)),
            )
        });
        let _ = execute!(io::stdout(), MoveTo(x, y), Show);
    }
}
