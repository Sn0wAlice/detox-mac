mod app;
mod modules;
mod ui;

use std::io;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = app::App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Erreur: {}", err);
    }

    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut app::App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Esc => {
                        if app.confirming {
                            app.cancel_confirm();
                        } else {
                            return Ok(());
                        }
                    }
                    KeyCode::Tab | KeyCode::Right => app.next_tab(),
                    KeyCode::BackTab | KeyCode::Left => app.prev_tab(),
                    KeyCode::Up | KeyCode::Char('k') => app.prev_task(),
                    KeyCode::Down | KeyCode::Char('j') => app.next_task(),
                    KeyCode::Enter => app.execute_current_task(),
                    KeyCode::Char(' ') => app.toggle_task(),
                    KeyCode::Char('r') => app.run_selected(),
                    KeyCode::Char('d') => app.toggle_dry_run(),
                    KeyCode::PageUp => app.scroll_log_up(),
                    KeyCode::PageDown => app.scroll_log_down(),
                    _ => {}
                }
            }
        }
    }
}
