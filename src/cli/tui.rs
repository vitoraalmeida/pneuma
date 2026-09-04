use std::io::{self, IsTerminal};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, terminal,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Alignment,
    widgets::{Block, Borders, Paragraph},
};

use super::error::CliError;

// Runs the TUI adapter without constructing host configuration or opening the database.
pub(super) fn run() -> Result<(), CliError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(CliError::TuiRequiresTerminal);
    }

    let mut terminal = TuiTerminal::open().map_err(|source| CliError::TuiTerminal { source })?;
    let result = terminal.run();
    let restored = terminal.restore();

    match (result, restored) {
        (Err(source), _) => Err(CliError::TuiTerminal { source }),
        (Ok(()), Err(source)) => Err(CliError::TuiTerminal { source }),
        (Ok(()), Ok(())) => Ok(()),
    }
}

// Owns the terminal mode while the TUI is active so every exit path restores it.
struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
}

impl TuiTerminal {
    fn open() -> io::Result<Self> {
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(
            terminal.backend_mut(),
            terminal::EnterAlternateScreen,
            cursor::Hide
        ) {
            let _ = execute!(
                terminal.backend_mut(),
                cursor::Show,
                terminal::LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }

        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn run(&mut self) -> io::Result<()> {
        loop {
            self.terminal.draw(draw_shell)?;
            if let Event::Key(event) = event::read()?
                && event.kind == KeyEventKind::Press
                && matches!(event.code, KeyCode::Char('q') | KeyCode::Esc)
            {
                return Ok(());
            }
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let screen = execute!(
            self.terminal.backend_mut(),
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
        let raw_mode = terminal::disable_raw_mode();
        match (screen, raw_mode) {
            (Err(error), _) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl Drop for TuiTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn draw_shell(frame: &mut ratatui::Frame<'_>) {
    let shell = Paragraph::new("Terminal interface setup complete\n\nPress q or Esc to quit.")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(" Pneuma "));
    frame.render_widget(shell, frame.area());
}
