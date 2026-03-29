mod navidrome;
mod lyrics;
mod config;
mod setup;

use chrono::{DateTime, Utc};
use crossbeam_channel::{bounded, select};
use crossterm::{
    event::{self, Event, KeyCode, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
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

use navidrome::{Track, Playlist, PlaylistTrack, get_current_track, get_playlists, get_playlist_tracks, jukebox_play};
use lyrics::SyncedLine;
use config::Config;

#[derive(Debug, PartialEq)]
enum AppView { NowPlaying, PlaylistBrowser }

#[derive(Debug, PartialEq)]
enum PlaylistFocus { Playlists, Tracks }

// ----------------------------------------
// App State
// ----------------------------------------
#[derive(Debug)]
struct AppState {
    config: Config,
    track: Option<Track>,

    duration_seconds: u32,
    start_timestamp_utc: Option<DateTime<Utc>>,
    progress_seconds: u32,
    progress: f32,

    raw_lyrics: Vec<String>,
    synced: Vec<SyncedLine>,
    cached_lines: Vec<Line<'static>>,

    current_line: u16,
    scroll: u16,
    current_word: Option<String>,

    status: String,

    view: AppView,
    playlists: Vec<Playlist>,
    playlist_cursor: usize,
    tracks: Vec<PlaylistTrack>,
    track_cursor: usize,
    playlist_focus: PlaylistFocus,
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

            status: "Press r to refresh • p playlists • q to quit • j/k to scroll".into(),

            view: AppView::NowPlaying,
            playlists: vec![],
            playlist_cursor: 0,
            tracks: vec![],
            track_cursor: 0,
            playlist_focus: PlaylistFocus::Playlists,
        }
    }
}

// ----------------------------------------
// Main entry
// ----------------------------------------
fn main() -> Result<(), Box<dyn std::error::Error>> {
    CombinedLogger::init(vec![
        WriteLogger::new(
            LevelFilter::Info,
            ConfigBuilder::new().build(),
            File::create("sonix_lyrics.log")?,
        )
    ])?;

    info!("Sonix Lyrics starting…");

    let config = Config::load_or_setup();

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

// ----------------------------------------
// Main TUI Loop
// ----------------------------------------
fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, cfg: Config) -> io::Result<()> {
    let mut app = AppState::new(cfg.clone());

    // metadata thread
    let (meta_tx, meta_rx) = bounded::<Track>(1);
    let refresh_interval = app.config.refresh_interval;

    std::thread::spawn(move || loop {
        match get_current_track(&cfg) {
            Ok(track) => { let _ = meta_tx.send(track); }
            Err(navidrome::NavidromeError::NoTrack) => {}
            Err(e) => error!("Navidrome error: {}", e),
        }
        std::thread::sleep(Duration::from_secs(refresh_interval));
    });

    // tick thread
    let (tick_tx, tick_rx) = bounded::<()>(1);
    std::thread::spawn(move || loop {
        let _ = tick_tx.send(());
        std::thread::sleep(Duration::from_millis(100));
    });

    let mut last_draw = Instant::now();

    loop {
        // metadata update
        if let Ok(track) = meta_rx.try_recv() {
            apply_track_update(&mut app, track);
        }

        // playback clock
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

        // karaoke word
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

        // auto scroll
        if !app.synced.is_empty() {
            let mut idx = 0;
            for (i, l) in app.synced.iter().enumerate() {
                if l.time_ms <= ms { idx = i; } else { break; }
            }

            if idx as u16 != app.current_line {
                app.current_line = idx as u16;
                app.scroll = app.current_line.saturating_sub(5);
            }
        }

        // draw UI
        if last_draw.elapsed() >= Duration::from_millis(33) {
            terminal.draw(|f| ui(f, &app))?;
            last_draw = Instant::now();
        }

        // handle input
        select! {
            recv(tick_rx) -> _ => {},
            default(Duration::from_millis(10)) => {
                if event::poll(Duration::from_millis(10))? {
                    match event::read()? {
                        Event::Key(key) => match key.code {
                            KeyCode::Char('q') => return Ok(()),

                            KeyCode::Char('p') => {
                                if app.view == AppView::NowPlaying {
                                    app.view = AppView::PlaylistBrowser;
                                    app.playlist_cursor = 0;
                                    app.track_cursor = 0;
                                    app.tracks = vec![];
                                    app.playlist_focus = PlaylistFocus::Playlists;
                                    match get_playlists(&app.config) {
                                        Ok(pls) => app.playlists = pls,
                                        Err(e) => error!("Playlists error: {}", e),
                                    }
                                } else {
                                    app.view = AppView::NowPlaying;
                                }
                            }

                            KeyCode::Esc => {
                                if app.view == AppView::PlaylistBrowser {
                                    app.view = AppView::NowPlaying;
                                }
                            }

                            KeyCode::Tab => {
                                if app.view == AppView::PlaylistBrowser {
                                    app.playlist_focus = match app.playlist_focus {
                                        PlaylistFocus::Playlists => PlaylistFocus::Tracks,
                                        PlaylistFocus::Tracks => PlaylistFocus::Playlists,
                                    };
                                }
                            }

                            KeyCode::Enter => {
                                if app.view == AppView::PlaylistBrowser {
                                    match app.playlist_focus {
                                        PlaylistFocus::Playlists => {
                                            if let Some(pl) = app.playlists.get(app.playlist_cursor) {
                                                match get_playlist_tracks(&app.config, &pl.id.clone()) {
                                                    Ok(tracks) => {
                                                        app.tracks = tracks;
                                                        app.track_cursor = 0;
                                                        app.playlist_focus = PlaylistFocus::Tracks;
                                                    }
                                                    Err(e) => error!("Tracks error: {}", e),
                                                }
                                            }
                                        }
                                        PlaylistFocus::Tracks => {
                                            if !app.tracks.is_empty() {
                                                let ids: Vec<String> = app.tracks[app.track_cursor..]
                                                    .iter()
                                                    .chain(app.tracks[..app.track_cursor].iter())
                                                    .map(|t| t.id.clone())
                                                    .collect();
                                                match jukebox_play(&app.config, &ids) {
                                                    Ok(()) => {
                                                        info!("Jukebox started");
                                                        app.view = AppView::NowPlaying;
                                                    }
                                                    Err(e) => error!("Jukebox error: {}", e),
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            KeyCode::Down | KeyCode::Char('j') => {
                                if app.view == AppView::PlaylistBrowser {
                                    match app.playlist_focus {
                                        PlaylistFocus::Playlists => {
                                            if app.playlist_cursor + 1 < app.playlists.len() {
                                                app.playlist_cursor += 1;
                                            }
                                        }
                                        PlaylistFocus::Tracks => {
                                            if app.track_cursor + 1 < app.tracks.len() {
                                                app.track_cursor += 1;
                                            }
                                        }
                                    }
                                } else {
                                    app.scroll += 1;
                                }
                            }

                            KeyCode::Up | KeyCode::Char('k') => {
                                if app.view == AppView::PlaylistBrowser {
                                    match app.playlist_focus {
                                        PlaylistFocus::Playlists => {
                                            app.playlist_cursor = app.playlist_cursor.saturating_sub(1);
                                        }
                                        PlaylistFocus::Tracks => {
                                            app.track_cursor = app.track_cursor.saturating_sub(1);
                                        }
                                    }
                                } else {
                                    app.scroll = app.scroll.saturating_sub(1);
                                }
                            }

                            KeyCode::Char('r') => {
                                if app.view == AppView::NowPlaying {
                                    if let Ok(track) = get_current_track(&app.config) {
                                        apply_track_update(&mut app, track);
                                    }
                                }
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

// ----------------------------------------
// Handle metadata update
// ----------------------------------------
fn apply_track_update(app: &mut AppState, track: Track) {
    let previous_title = app.track.as_ref().map(|t| t.title.clone());
    let previous_timestamp = app.track.as_ref().and_then(|t| t.played_timestamp);

    let song_changed = previous_title != Some(track.title.clone());
    let restarted = previous_timestamp != track.played_timestamp;

    app.track = Some(track.clone());
    app.duration_seconds = track.duration;
    app.start_timestamp_utc = track.played_timestamp;

    if song_changed || restarted {
        app.progress_seconds = 0;
        app.progress = 0.0;
    }

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

// ----------------------------------------
// Cache lyric lines for TUI
// ----------------------------------------
fn cache_lines(raw: &[String]) -> Vec<Line<'static>> {
    raw.iter()
        .map(|l| Line::from(Box::leak(l.clone().into_boxed_str()).to_string()))
        .collect()
}

// ----------------------------------------
// UI
// ----------------------------------------
fn ui(f: &mut Frame, app: &AppState) {
    match app.view {
        AppView::NowPlaying => {
            let area = f.area();
            let layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
                .split(area);
            f.render_widget(render_left(app), layout[0]);
            f.render_widget(render_lyrics(app), layout[1]);
        }
        AppView::PlaylistBrowser => render_playlists(f, app),
    }
}

fn render_playlists(f: &mut Frame, app: &AppState) {
    let area = f.area();

    // Split: 30% playlists, 70% tracks
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    // ---- left: playlist list ----
    let pl_focus = app.playlist_focus == PlaylistFocus::Playlists;
    let pl_lines: Vec<Line> = if app.playlists.is_empty() {
        vec![Line::from(Span::styled("Loading…", Style::default().fg(Color::DarkGray)))]
    } else {
        app.playlists.iter().enumerate().map(|(i, p)| {
            let label = format!(" {} ({} tracks)", p.name, p.song_count);
            if i == app.playlist_cursor && pl_focus {
                Line::from(Span::styled(label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if i == app.playlist_cursor {
                Line::from(Span::styled(label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            } else {
                Line::from(Span::raw(label))
            }
        }).collect()
    };

    let pl_border = if pl_focus { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) };
    f.render_widget(
        Paragraph::new(pl_lines)
            .block(Block::default().borders(Borders::ALL).title("Playlists").border_style(pl_border))
            .wrap(Wrap { trim: false }),
        layout[0],
    );

    // ---- right: track list ----
    let tr_focus = app.playlist_focus == PlaylistFocus::Tracks;
    let tr_lines: Vec<Line> = if app.tracks.is_empty() {
        vec![Line::from(Span::styled(
            "Select a playlist and press Enter",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.tracks.iter().enumerate().map(|(i, t)| {
            let mins = t.duration / 60;
            let secs = t.duration % 60;
            let label = format!(" {}. {} — {} ({:02}:{:02})", i + 1, t.title, t.artist, mins, secs);
            if i == app.track_cursor && tr_focus {
                Line::from(Span::styled(label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if i == app.track_cursor {
                Line::from(Span::styled(label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            } else {
                Line::from(Span::raw(label))
            }
        }).collect()
    };

    let tr_border = if tr_focus { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) };
    let track_title = if let Some(pl) = app.playlists.get(app.playlist_cursor) {
        format!("Tracks — {}", pl.name)
    } else {
        "Tracks".to_string()
    };

    // scroll so selected track stays visible
    let tr_scroll = if tr_focus && app.track_cursor > 5 { (app.track_cursor - 5) as u16 } else { 0 };

    f.render_widget(
        Paragraph::new(tr_lines)
            .block(Block::default().borders(Borders::ALL).title(track_title).border_style(tr_border))
            .scroll((tr_scroll, 0))
            .wrap(Wrap { trim: false }),
        layout[1],
    );
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

        if !t.album.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Album: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(t.album.clone()),
            ]));
        }

        // // progress bar
        // if app.duration_seconds > 0 {
        //     let width = 20;
        //     let filled = (app.progress * width as f32).round() as usize;
        //     let bar = [
        //         "█".repeat(filled.min(width)),
        //         "░".repeat(width - filled.min(width))
        //     ].join("");

        //     lines.push(Line::default());
        //     lines.push(Line::from(
        //         Span::styled(
        //             format!(
        //                 "[{}] {:02}:{:02} / {:02}:{:02}",
        //                 bar,
        //                 app.progress_seconds / 60,
        //                 app.progress_seconds % 60,
        //                 app.duration_seconds / 60,
        //                 app.duration_seconds % 60
        //             ),
        //             Style::default().fg(Color::Green)
        //         )
        //     ));
        // }

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
            let display = app
                .synced
                .get(app.current_line as usize)
                .map(|s| s.text.clone())
                .unwrap_or_else(|| line.to_string());

            *line = Line::from(
                Span::styled(
                    display,
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
