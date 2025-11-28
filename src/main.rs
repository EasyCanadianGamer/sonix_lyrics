mod navidrome;
mod lyrics;
mod config;

use chrono::{DateTime, Utc};
use crossbeam_channel::{bounded, select};
use crossterm::{
    event::{self, Event, KeyCode, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};

use log::{info, error};
use ratatui::{
    backend::CrosstermBackend,
    Terminal, Frame,
    layout::{Layout, Direction, Constraint},
    widgets::{Paragraph, Block, Borders, Wrap},
    style::{Color, Style, Modifier},
    text::{Line, Span},
};
use simplelog::*;
use std::fs::File;
use std::io;
use std::time::{Duration, Instant};

use navidrome::{Track, get_current_track};
use lyrics::{LyricsData};
use lyrics::{SyncedLine, KaraokeWord};
use config::Config;


/// ============================================================
/// Application State
/// ============================================================

#[derive(Debug)]
struct AppState {
    config: Config,

    track: Option<Track>,

    // timing
    duration_seconds: u32,
    start_timestamp_utc: Option<DateTime<Utc>>,
    progress_seconds: u32,
    progress: f32,

    // lyrics
    raw_lyrics: Vec<String>,
    synced: Vec<SyncedLine>,
    cached_lines: Vec<Line<'static>>,

    // lyric scroll
    current_line: u16,
    scroll: u16,

    // karaoke word
    current_word: Option<String>,

    status: String,
}

impl AppState {
    fn new(config: Config) -> Self {
        Self {
            config,
            track: None,

            duration_seconds: 0,
            start_timestamp_utc: None,
            progress_seconds: 0,
            progress: 0.0,

            raw_lyrics: vec!["No lyrics loaded".into()],
            synced: vec![],
            cached_lines: vec![],

            current_line: 0,
            scroll: 0,
            current_word: None,

            status: "Press r to refresh · q to quit · j/k scroll".into(),
        }
    }
}



/// ============================================================
/// MAIN
/// ============================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {

    // Logging
    CombinedLogger::init(vec![
        WriteLogger::new(
            LevelFilter::Info,
            ConfigBuilder::new().build(),
            File::create("sonix_lyrics.log")?,
        )
    ])?;

    info!("Sonix Lyrics starting…");

    // Load config
    let config = Config::load();

    enable_raw_mode()?;
    let mut stdout = io::stdout();

    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, config);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    terminal.show_cursor()?;

    if let Err(e) = result {
        error!("Fatal: {}", e);
        println!("Error: {}", e);
    }

    Ok(())
}



/// ============================================================
/// Main TUI Loop
/// ============================================================

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, cfg: Config) -> io::Result<()> {
    let mut app = AppState::new(cfg.clone());

    // Channel for metadata thread
    let (meta_tx, meta_rx) = bounded::<Track>(1);

    // Background metadata fetcher
    let refresh_interval = app.config.refresh_interval;
    std::thread::spawn(move || loop {
        match get_current_track(&cfg) {
            Ok(track) => { let _ = meta_tx.send(track); }
            Err(e) => error!("Navidrome error: {}", e),
        }

        std::thread::sleep(Duration::from_secs(refresh_interval));
    });

    // Tick channel (100ms)
    let (tick_tx, tick_rx) = bounded::<()>(1);
    std::thread::spawn(move || loop {
        let _ = tick_tx.send(());
        std::thread::sleep(Duration::from_millis(100));
    });

    let mut last_draw = Instant::now();

    loop {
        // -----------------------------
        // Metadata update
        // -----------------------------
        if let Ok(track) = meta_rx.try_recv() {
            apply_track_update(&mut app, track);
        }

        // -----------------------------
        // Playback clock
        // -----------------------------
        let pos = if let Some(start) = app.start_timestamp_utc {
            let now = Utc::now();
            let diff = now.signed_duration_since(start).num_milliseconds();
            (diff as f32 / 1000.0).clamp(0.0, app.duration_seconds as f32)
        } else {
            0.0
        };

        app.progress_seconds = pos.floor() as u32;
        app.progress = if app.duration_seconds > 0 {
             pos / app.duration_seconds as f32
        } else { 0.0 };

        // -----------------------------
        // Karaoke word highlight
        // -----------------------------
        app.current_word = None;
        let ms = (pos * 1000.0) as u32;

        if app.config.karaoke_enabled {
            for line in &app.synced {
                if line.time_ms <= ms {
                    for w in &line.words {
                        if w.time_ms <= ms {
                            app.current_word = Some(w.word.clone());
                        }
                    }
                }
            }
        }

        // -----------------------------
        // Auto-scroll synced lyrics
        // -----------------------------
        if !app.synced.is_empty() {
            let mut idx = 0;

            for (i, l) in app.synced.iter().enumerate() {
                if l.time_ms <= ms { idx = i; }
                else { break; }
            }

            if idx as u16 != app.current_line {
                app.current_line = idx as u16;
                app.scroll = app.current_line.saturating_sub(5);
            }
        }

        // -----------------------------
        // Draw UI
        // -----------------------------
        if last_draw.elapsed() >= Duration::from_millis(33) {
            terminal.draw(|f| ui(f, &app))?;
            last_draw = Instant::now();
        }

        // -----------------------------
        // Input
        // -----------------------------
        select! {
            recv(tick_rx) -> _ => {},

            default(Duration::from_millis(10)) => {
                if event::poll(Duration::from_millis(10))? {
                    match event::read()? {
                        Event::Key(key) => match key.code {
                            KeyCode::Char('q') => return Ok(()),

                            KeyCode::Char('r') => {
                                if let Ok(track) = get_current_track(&app.config) {
                                    apply_track_update(&mut app, track);
                                }
                            }

                            KeyCode::Down | KeyCode::Char('j') => app.scroll += 1,
                            KeyCode::Up   | KeyCode::Char('k') => {
                                app.scroll = app.scroll.saturating_sub(1)
                            }

                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }
    }
}



/// ============================================================
/// When track metadata changes
/// ============================================================

fn apply_track_update(app: &mut AppState, track: Track) {
    let previous_title = app.track.as_ref().map(|t| t.title.clone());
    let previous_timestamp = app.track.as_ref().and_then(|t| t.played_timestamp);

    let song_changed = previous_title != Some(track.title.clone());
    let restarted = previous_timestamp != track.played_timestamp;

    // Always update state
    app.track = Some(track.clone());
    app.duration_seconds = track.duration;
    app.start_timestamp_utc = track.played_timestamp;

    // Reset timer when:
    //  ✔ new song
    //  ✔ same song looped
    if song_changed || restarted {
        app.progress_seconds = 0;
        app.progress = 0.0;
    }

    // Fetch lyrics ONLY if changed
    if song_changed {
        match lyrics::fetch_lyrics(&track.artist, &track.title) {
            Ok(ld) => {
                app.raw_lyrics = ld.lines.clone();
                app.synced = ld.synced.clone();
                app.cached_lines = cache_lines(&app.raw_lyrics);

                app.current_line = 0;
                app.scroll = 0;

                app.status = format!("Now playing: {}", track.title);
                info!("Loaded lyrics for {}", track.title);
            }
            Err(e) => {
                app.raw_lyrics = vec!["No lyrics found".into()];
                app.synced.clear();
                app.cached_lines = cache_lines(&app.raw_lyrics);

                app.status = format!("Lyrics not found ({})", e);
                error!("Lyrics error: {}", e);
            }
        }
    }
}



/// ============================================================
/// Cache lyric lines for ratatui
/// ============================================================

fn cache_lines(raw: &[String]) -> Vec<Line<'static>> {
    raw.iter()
        .map(|l| Line::from(Box::leak(l.clone().into_boxed_str()).to_string()))
        .collect()
}



/// ============================================================
/// UI
/// ============================================================

fn ui(f: &mut Frame, app: &AppState) {
    let area = f.area();

    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    f.render_widget(render_left(app), layout[0]);
    f.render_widget(render_lyrics(app), layout[1]);
}

fn render_left(app: &AppState) -> Paragraph<'static> {
    let mut lines = vec![];

    if let Some(t) = &app.track {
        lines.push(Line::from(vec![
            Span::styled("Title: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(t.title.clone()),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Artist: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(t.artist.clone()),
        ]));

        // Progress bar
        // if app.duration_seconds > 0 {
        //     let width = 22;
        //     let filled = (app.progress * width as f32) as usize;

        //     let bar = format!(
        //         "[{}{}]",
        //         "█".repeat(filled.min(width)),
        //         "░".repeat(width - filled.min(width))
        //     );

        //     lines.push(Line::default());
        //     lines.push(Line::from(Span::styled(
        //         format!(
        //             "{} {:02}:{:02} / {:02}:{:02}",
        //             bar,
        //             app.progress_seconds / 60,
        //             app.progress_seconds % 60,
        //             app.duration_seconds / 60,
        //             app.duration_seconds % 60,
        //         ),
        //         Style::default().fg(Color::Green),
        //     )));
        // }

        // Karaoke active word
        if let Some(w) = &app.current_word {
            lines.push(Line::default());
            lines.push(Line::from(
                Span::styled(
                    format!("♪ {} ♪", w),
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                )
            ));
        }
    }

    // Status
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        app.status.clone(),
        Style::default().fg(Color::Yellow),
    )));

    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Track Info")
                .border_style(Style::default().fg(Color::Green))
        )
}

fn render_lyrics(app: &AppState) -> Paragraph<'static> {
    let mut lines = app.cached_lines.clone();

if app.config.karaoke_enabled {
    if let Some(line) = lines.get_mut(app.current_line as usize) {
        *line = Line::from(
            Span::styled(
                line.to_string(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            )
        );
    }
}


    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Lyrics")
                .border_style(Style::default().fg(Color::Blue))
        )
        .scroll((app.scroll, 0))
        .wrap(Wrap { trim: false })
}
