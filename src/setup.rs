// src/setup.rs
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
    widgets::{Block, Borders, Paragraph},
    layout::{Layout, Constraint, Direction},
    style::{Style, Color, Modifier},
    text::{Span, Line},
};
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use std::io::{self};

use md5;
use rand::{thread_rng, Rng};

use crate::config::Config;

#[derive(Debug)]
enum Field {
    Url,
    User,
    Pass,
}

pub fn run_setup_wizard() -> Config {
    let mut url = String::new();
    let mut user = String::new();
    let mut pass = String::new();

    let mut field = Field::Url;
    let mut cursor: usize = 0;
    enable_raw_mode().unwrap();
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    loop {
        terminal.draw(|f| {
            let size = f.area();

            let block = Block::default()
                .title("Sonix Lyrics — First-Time Setup")
                .borders(Borders::ALL);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(2),
                    Constraint::Length(1),
                ])
                .split(size);

            f.render_widget(block, size);

            let url_disp = format!("[ {} ]", url);
            let usr_disp = format!("[ {} ]", user);
            let pwd_disp = format!("[ {} ]", "*".repeat(pass.len()));

            f.render_widget(Paragraph::new(Line::from(vec![
                Span::styled("Navidrome URL: ", bold()),
                Span::raw(url_disp),
            ])), layout[0]);

            f.render_widget(Paragraph::new(Line::from(vec![
                Span::styled("Username:      ", bold()),
                Span::raw(usr_disp),
            ])), layout[1]);

            f.render_widget(Paragraph::new(Line::from(vec![
                Span::styled("Password:      ", bold()),
                Span::raw(pwd_disp),
            ])), layout[2]);

            f.render_widget(
                Paragraph::new("TAB: Next | ENTER: Confirm | ESC: Cancel")
                    .style(Style::default().fg(Color::Yellow)),
                layout[4],
            );
        }).unwrap();

        if let Event::Key(key) = event::read().unwrap() {
            match key.code {
                KeyCode::Esc => break,
                KeyCode::Tab => {
                    field = match field {
                        Field::Url => Field::User,
                        Field::User => Field::Pass,
                        Field::Pass => Field::Url,
                    };
                    cursor = 0;
                }
                KeyCode::Left => cursor = cursor.saturating_sub(1),
                KeyCode::Right => cursor += 1,

                KeyCode::Backspace => match field {
                    Field::Url => { if cursor > 0 { url.remove(cursor - 1); cursor -= 1; } }
                    Field::User => { if cursor > 0 { user.remove(cursor - 1); cursor -= 1; } }
                    Field::Pass => { if cursor > 0 { pass.remove(cursor - 1); cursor -= 1; } }
                }

                KeyCode::Enter => {
                    // Only finalize when password field is focused
                    if matches!(field, Field::Pass) {
                        disable_raw_mode().unwrap();
                        execute!(terminal.backend_mut(), LeaveAlternateScreen).unwrap();

                        // Generate salt + token
                        let salt = generate_salt();
                        let token = format!("{:x}", md5::compute(format!("{}{}", pass, salt)));

                        return Config {
                            navidrome_url: url,
                            navidrome_user: user,
                            navidrome_token: token,
                            navidrome_salt: salt,
                            refresh_interval: 2,
                            karaoke_enabled: false,
                        };
                    } else {
                        field = match field {
                            Field::Url => Field::User,
                            Field::User => Field::Pass,
                            Field::Pass => Field::Pass,
                        };
                        cursor = 0;
                    }
                }

                KeyCode::Char(c) => match field {
                    Field::Url => {
                        url.insert(cursor, c);
                        cursor += 1;
                    }
                    Field::User => {
                        user.insert(cursor, c);
                        cursor += 1;
                    }
                    Field::Pass => {
                        pass.insert(cursor, c);
                        cursor += 1;
                    }
                }

                _ => {}
            }
        }
    }

    // If ESC or cancel → exit process safely
    disable_raw_mode().unwrap();
    execute!(io::stdout(), LeaveAlternateScreen).unwrap();
    println!("Setup aborted.");

    std::process::exit(0);
}

fn generate_salt() -> String {
    let mut rng = thread_rng();
    (0..12)
        .map(|_| rng.gen_range(b'a'..=b'z') as char)
        .collect()
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
