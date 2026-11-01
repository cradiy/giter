mod app;
mod conflict;
mod git;
mod highlight;
mod input;
mod theme;
mod tree;
mod ui;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
};

use app::{Action, App};
use git::Repo;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Path inside a Git working tree
    #[arg(default_value = ".")]
    path: PathBuf,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let repo = Repo::open(&args.path)?;
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "giter requires an interactive terminal"
    );
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
        panic_hook(info);
    }));
    execute!(io::stdout(), EnableBracketedPaste)?;
    let mut app = App::new(repo);
    let mut dirty = true;
    loop {
        dirty |= app.poll();
        if dirty {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(80))? {
            dirty = true;
            match app.event(event::read()?) {
                Action::Quit => break,
                Action::None => {}
                Action::Editor(path) => match app.repo.editor(&path) {
                    Ok(mut command) => {
                        execute!(io::stdout(), DisableBracketedPaste)?;
                        ratatui::restore();
                        let result = command.status().context("Cannot launch editor");
                        terminal = ratatui::init();
                        execute!(io::stdout(), EnableBracketedPaste)?;
                        terminal.clear()?;
                        match result {
                            Ok(status) if status.success() => {
                                app.notice("Editor closed", false);
                                app.refresh();
                            }
                            Ok(status) => {
                                app.notice(format!("Editor exited with {status}"), true);
                                app.refresh();
                            }
                            Err(error) => app.notice(format!("{error:#}"), true),
                        }
                    }
                    Err(error) => app.notice(format!("{error:#}"), true),
                },
            }
        }
    }
    Ok(())
}
