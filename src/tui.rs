//! The terminal side of the application: event loop, screen setup and
//! handing the screen over to `$EDITOR`.

use std::io::{stdout, Write};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;

use crate::app::{App, EditRequest, Effect};
use crate::ui;

/// How long one wait for input lasts.
const TICK: Duration = Duration::from_millis(120);

/// The terminal in TUI mode (raw, alternate screen, optional mouse).
/// Dropping it always gives the terminal back, also on errors.
struct Screen {
    terminal: DefaultTerminal,
    mouse: bool,
}

impl Screen {
    fn enter(mouse: bool) -> Result<Self> {
        let terminal = ratatui::try_init().context("cannot initialize the terminal")?;
        let screen = Self { terminal, mouse };
        screen.capture_mouse(true);
        Ok(screen)
    }

    fn capture_mouse(&self, on: bool) {
        if self.mouse {
            let _ = if on {
                execute!(stdout(), EnableMouseCapture)
            } else {
                execute!(stdout(), DisableMouseCapture)
            };
        }
    }

    /// Hand the terminal to another program for the duration of `f`.
    fn suspended<T>(&mut self, f: impl FnOnce() -> T) -> Result<T> {
        self.capture_mouse(false);
        ratatui::restore();
        let value = f();
        self.terminal = ratatui::try_init().context("cannot reclaim the terminal")?;
        self.capture_mouse(true);
        self.terminal.clear()?;
        Ok(value)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.capture_mouse(false);
        ratatui::restore();
    }
}

/// Run the interactive session until the user quits.
pub fn run(app: &mut App) -> Result<()> {
    let mouse = app.config.ui.mouse;
    let mut screen = Screen::enter(mouse)?;
    let mut redraw = true;
    loop {
        if redraw {
            screen.terminal.draw(|frame| ui::draw(frame, app))?;
            redraw = false;
        }
        let mut effect = Effect::None;
        if event::poll(TICK)? {
            // Drain everything that is queued so a burst of input (key
            // repeat, mouse motion) costs a single redraw.
            loop {
                effect = app.on_event(event::read()?);
                redraw = true;
                if effect != Effect::None || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        match effect {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Edit(request) => {
                let edited = screen.suspended(|| edit_in_editor(&request))?;
                app.finish_edit(&request, edited);
                redraw = true;
            }
        }
    }
}

/// `$MCR_EDITOR`, `$VISUAL`, `$EDITOR`, then `vi`.
fn editor_command() -> String {
    ["MCR_EDITOR", "VISUAL", "EDITOR"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_owned())
}

/// Let the user edit the request's text; returns the edited text.
fn edit_in_editor(request: &EditRequest) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("mcr-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = match &request.extension {
        Some(ext) => format!("hunk.{ext}"),
        None => "hunk".to_owned(),
    };
    let path = dir.join(name);
    let result = (|| {
        std::fs::File::create(&path)
            .and_then(|mut file| file.write_all(request.text.as_bytes()))
            .map_err(|e| e.to_string())?;
        let editor = editor_command();
        // Like git: the editor setting is a shell snippet ("code --wait").
        let status = if cfg!(windows) {
            Command::new("cmd")
                .arg("/C")
                .arg(format!("{editor} \"{}\"", path.display()))
                .status()
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(format!("{editor} \"$1\""))
                .arg("mcr-editor")
                .arg(&path)
                .status()
        }
        .map_err(|e| format!("cannot run `{editor}`: {e}"))?;
        if !status.success() {
            return Err(format!("`{editor}` exited with {status}"));
        }
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}
