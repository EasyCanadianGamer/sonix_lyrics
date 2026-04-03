mod config;
mod lyrics;
mod navidrome;
mod setup;

use chrono::{DateTime, Utc};
use crossbeam_channel::{bounded, select};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::GenericImageView;
use log::{error, info};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use simplelog::*;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use config::Config;
use lyrics::SyncedLine;
use navidrome::{get_playlist_tracks, get_playlists, Playlist, PlaylistTrack};

const MPV_SOCK: &str = "/tmp/sonix_lyrics_mpv.sock";

// ----------------------------------------
// Enums
// ----------------------------------------
#[derive(Debug, PartialEq, Clone, Copy)]
enum LoopMode {
    Off,
    Track,
    Playlist,
}

impl LoopMode {
    fn next(self) -> Self {
        match self {
            LoopMode::Off => LoopMode::Track,
            LoopMode::Track => LoopMode::Playlist,
            LoopMode::Playlist => LoopMode::Off,
        }
    }
    fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "Off",
            LoopMode::Track => "Track",
            LoopMode::Playlist => "Playlist",
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum AppView {
    NowPlaying,
    Queue,
    Playlists,
    Settings,
}

#[derive(Debug, PartialEq)]
enum PlaylistFocus {
    Playlists,
    Tracks,
}

// ----------------------------------------
// App State
// ----------------------------------------
struct AppState {
    config: Config,

    title: String,
    artist: String,
    album: String,
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

    cover_art_lines: Vec<Line<'static>>,  // halfblock fallback
    cover_art_kitty: Option<(Vec<u8>, u32, u32)>, // (rgba_bytes, img_w, img_h) for Kitty
    is_kitty: bool,

    status: String,
    loop_mode: LoopMode,

    view: AppView,

    // Playlists (F3)
    playlists: Vec<Playlist>,
    playlist_cursor: usize,
    tracks: Vec<PlaylistTrack>,
    track_cursor: usize,
    playlist_focus: PlaylistFocus,

    // Queue (F2)
    queue_cursor: usize,

    // Playback
    jukebox_playing: bool,
    jukebox_gain: f32,
    jukebox_index: i32,
    mpv_process: Option<std::process::Child>,

    // Settings (F4) — transient editable values
    settings_cursor: usize,
    settings_editing: bool,
    settings_buf: String,
    settings_url: String,
    settings_user: String,
    settings_refresh: String,
    settings_karaoke: bool,

    show_help: bool,
}

impl AppState {
    fn new(config: Config) -> Self {
        let settings_url = config.navidrome_url.clone();
        let settings_user = config.navidrome_user.clone();
        let settings_refresh = config.refresh_interval.to_string();
        let settings_karaoke = config.karaoke_enabled;

        Self {
            config,

            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration_seconds: 0,
            start_timestamp_utc: None,
            progress_seconds: 0,
            progress: 0.0,

            raw_lyrics: vec!["Press F3 or 'p' to open your playlists.".into()],
            synced: vec![],
            cached_lines: vec![Line::from("Press F3 or 'p' to open your playlists.")],
            current_line: 0,
            scroll: 0,
            current_word: None,

            cover_art_lines: vec![],
            cover_art_kitty: None,
            is_kitty: std::env::var("TERM").map(|t| t == "xterm-kitty").unwrap_or(false)
                || std::env::var("KITTY_WINDOW_ID").is_ok(),

            status: "F1-F4=views  Space=play/pause  [/]=prev/next  l=loop  ?=help  q=quit".into(),
            loop_mode: LoopMode::Off,

            view: AppView::NowPlaying,

            playlists: vec![],
            playlist_cursor: 0,
            tracks: vec![],
            track_cursor: 0,
            playlist_focus: PlaylistFocus::Playlists,

            queue_cursor: 0,

            jukebox_playing: false,
            jukebox_gain: 0.7,
            jukebox_index: 0,
            mpv_process: None,

            settings_cursor: 0,
            settings_editing: false,
            settings_buf: String::new(),
            settings_url,
            settings_user,
            settings_refresh,
            settings_karaoke,

            show_help: false,
        }
    }

    fn sync_settings_from_config(&mut self) {
        self.settings_url = self.config.navidrome_url.clone();
        self.settings_user = self.config.navidrome_user.clone();
        self.settings_refresh = self.config.refresh_interval.to_string();
        self.settings_karaoke = self.config.karaoke_enabled;
    }

    fn save_settings(&mut self) {
        self.config.navidrome_url = self.settings_url.trim().to_string();
        self.config.navidrome_user = self.settings_user.trim().to_string();
        self.config.refresh_interval = self.settings_refresh.trim().parse().unwrap_or(2);
        self.config.karaoke_enabled = self.settings_karaoke;
        self.config.save();
        self.status = "Settings saved.".into();
        info!("Settings saved");
    }
}

// ----------------------------------------
// mpv helpers
// ----------------------------------------
fn spawn_mpv(url: &str) -> Option<std::process::Child> {
    std::process::Command::new("mpv")
        .args([
            "--no-video",
            "--really-quiet",
            &format!("--input-ipc-server={}", MPV_SOCK),
            url,
        ])
        .spawn()
        .map_err(|e| error!("Failed to spawn mpv: {}", e))
        .ok()
}

fn kill_mpv(process: &mut Option<std::process::Child>) {
    if let Some(ref mut child) = process {
        let _ = child.kill();
        let _ = child.wait();
    }
    *process = None;
    let _ = std::fs::remove_file(MPV_SOCK);
}

fn mpv_ipc(cmd: &str) {
    if let Ok(mut sock) = UnixStream::connect(MPV_SOCK) {
        let _ = sock.write_all(cmd.as_bytes());
    }
}

// ----------------------------------------
// Cover art
// ----------------------------------------

/// Halfblock rendering for non-Kitty terminals.
fn render_cover_art_halfblock(img: &image::DynamicImage, cols: u32, rows: u32) -> Vec<Line<'static>> {
    let img = img.resize_exact(cols, rows * 2, image::imageops::FilterType::Nearest);
    (0..rows)
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..cols)
                .map(|col| {
                    let top = img.get_pixel(col, row * 2);
                    let bot = img.get_pixel(col, row * 2 + 1);
                    Span::styled(
                        "▄",
                        Style::default()
                            .fg(Color::Rgb(bot[0], bot[1], bot[2]))
                            .bg(Color::Rgb(top[0], top[1], top[2])),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

fn b64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let n = match c.len() {
            3 => (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32,
            2 => (c[0] as u32) << 16 | (c[1] as u32) << 8,
            _ => (c[0] as u32) << 16,
        };
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if c.len() >= 2 { A[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if c.len() == 3 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Send a Kitty graphics protocol image to the terminal at a specific cell position.
/// Must be called AFTER terminal.draw() since Ratatui redraws erase the image each frame.
fn kitty_draw(rgba: &[u8], img_w: u32, img_h: u32, cell_x: u16, cell_y: u16, cell_cols: u16, cell_rows: u16) {
    let b64 = b64_encode(rgba);
    let mut stdout = io::stdout().lock();
    // Move cursor to the target cell (ANSI cursor position is 1-indexed)
    let _ = write!(stdout, "\x1b[{};{}H", cell_y + 1, cell_x + 1);
    // Kitty graphics protocol: transmit raw RGBA (f=32), display in cell_cols×cell_rows cells
    let chunks: Vec<&str> = b64
        .as_bytes()
        .chunks(4096)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    let total = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < total { 1 } else { 0 };
        if i == 0 {
            let _ = write!(
                stdout,
                "\x1b_Ga=T,f=32,s={},v={},c={},r={},q=2,m={};{}\x1b\\",
                img_w, img_h, cell_cols, cell_rows, more, chunk
            );
        } else {
            let _ = write!(stdout, "\x1b_Gm={};{}\x1b\\", more, chunk);
        }
    }
    let _ = stdout.flush();
}

// ----------------------------------------
// play_track
// ----------------------------------------
fn play_track(app: &mut AppState, idx: usize) {
    if idx >= app.tracks.len() {
        return;
    }
    kill_mpv(&mut app.mpv_process);

    let pt = app.tracks[idx].clone();
    let url = navidrome::stream_url(&app.config, &pt.id);

    app.mpv_process = spawn_mpv(&url);
    app.jukebox_playing = app.mpv_process.is_some();
    app.jukebox_index = idx as i32;
    app.queue_cursor = idx;

    app.title = pt.title.clone();
    app.artist = pt.artist.clone();
    app.album = String::new();
    app.duration_seconds = pt.duration;
    app.start_timestamp_utc = Some(Utc::now());
    app.progress_seconds = 0;
    app.progress = 0.0;
    app.current_line = 0;
    app.scroll = 0;

    // fetch cover art
    let art_img = pt.cover_art_id.as_deref()
        .and_then(|id| navidrome::fetch_cover_art_bytes(&app.config, id))
        .and_then(|bytes| image::load_from_memory(&bytes).ok());

    if app.is_kitty {
        app.cover_art_kitty = art_img.as_ref().map(|img| {
            let resized = img.resize_exact(220, 160, image::imageops::FilterType::Lanczos3);
            let rgba = resized.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            (rgba.into_raw(), w, h)
        });
        app.cover_art_lines = vec![];
    } else {
        app.cover_art_kitty = None;
        app.cover_art_lines = art_img
            .map(|img| render_cover_art_halfblock(&img, 22, 8))
            .unwrap_or_default();
    }

    // fetch lyrics
    match lyrics::fetch_lyrics(&pt.artist, &pt.title) {
        Ok(ld) => {
            app.raw_lyrics = ld.lines.clone();
            app.synced = ld.synced.clone();
            app.cached_lines = cache_lines(&app.raw_lyrics);
            app.status = format!("Now playing: {} — {}", pt.artist, pt.title);
            info!("Loaded lyrics for {}", pt.title);
        }
        Err(e) => {
            app.raw_lyrics = vec!["No lyrics found".into()];
            app.synced.clear();
            app.cached_lines = cache_lines(&app.raw_lyrics);
            app.status = format!("No lyrics ({})", e);
        }
    }
}

// ----------------------------------------
// handle_track_end — YOUR CONTRIBUTION
// ----------------------------------------
// When mpv finishes playing a track, this decides what happens next.
//
// Fields available:
//   app.loop_mode   — LoopMode::Off | Track | Playlist
//   app.jukebox_index — current track index (i32)
//   app.tracks      — the loaded track list
//   play_track(app, idx) — call to play a track at index
//
// Trade-offs to consider:
//   - LoopMode::Off: advance to next automatically, or stop entirely?
//   - LoopMode::Playlist: wrap index back to 0 when at the end
//   - LoopMode::Track: restart same index
//
// Implement the 3 match arms below (5-10 lines):
fn handle_track_end(app: &mut AppState) {
    let len = app.tracks.len();
    if len == 0 {
        return;
    }
    match app.loop_mode {
        LoopMode::Off => {
            let next = app.jukebox_index + 1;
            if next < len as i32 {
                play_track(app, next as usize);
            }
        }
        LoopMode::Track => {
            let idx = app.jukebox_index as usize;
            play_track(app, idx);
        }
        LoopMode::Playlist => {
            let next = (app.jukebox_index + 1) as usize % len;
            play_track(app, next);
        }
    }
}

// ----------------------------------------
// Main entry
// ----------------------------------------
fn main() -> Result<(), Box<dyn std::error::Error>> {
    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Info,
        ConfigBuilder::new().build(),
        File::create("sonix_lyrics.log")?,
    )])?;

    info!("Sonix Lyrics starting…");

    let config = Config::load_or_setup();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, config);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
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
    let mut app = AppState::new(cfg);

    let (tick_tx, tick_rx) = bounded::<()>(1);
    std::thread::spawn(move || loop {
        let _ = tick_tx.send(());
        std::thread::sleep(Duration::from_millis(100));
    });

    let mut last_draw = Instant::now();

    loop {
        // playback clock
        let pos = if let Some(start) = app.start_timestamp_utc {
            let diff = Utc::now().signed_duration_since(start).num_milliseconds();
            (diff as f32 / 1000.0).clamp(0.0, app.duration_seconds as f32)
        } else {
            0.0
        };

        app.progress_seconds = pos.floor() as u32;
        app.progress = if app.duration_seconds > 0 { pos / app.duration_seconds as f32 } else { 0.0 };

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

        // reap mpv and trigger loop logic
        let mpv_finished = if let Some(ref mut child) = app.mpv_process {
            matches!(child.try_wait(), Ok(Some(_)))
        } else { false };

        if mpv_finished {
            app.mpv_process = None;
            app.jukebox_playing = false;
            handle_track_end(&mut app);
        }

        if last_draw.elapsed() >= Duration::from_millis(33) {
            terminal.draw(|f| ui(f, &app))?;
            // Kitty image must be resent after every draw (Ratatui cell writes erase it)
            if app.is_kitty {
                if let Some((ref rgba, img_w, img_h)) = app.cover_art_kitty {
                    let size = terminal.size()?;
                    let left_w = size.width * 35 / 100;
                    kitty_draw(rgba, img_w, img_h, 0, 1, left_w, 10);
                }
            }
            last_draw = Instant::now();
        }

        select! {
            recv(tick_rx) -> _ => {},
            default(Duration::from_millis(10)) => {
                if event::poll(Duration::from_millis(10))? {
                    if let Event::Key(key) = event::read()? {
                        // settings edit mode eats all input
                        if app.settings_editing {
                            match key.code {
                                KeyCode::Esc => {
                                    app.settings_editing = false;
                                    app.sync_settings_from_config();
                                }
                                KeyCode::Enter => {
                                    app.settings_editing = false;
                                    // apply buf to the right field
                                    match app.settings_cursor {
                                        0 => app.settings_url = app.settings_buf.clone(),
                                        1 => app.settings_user = app.settings_buf.clone(),
                                        2 => app.settings_refresh = app.settings_buf.clone(),
                                        _ => {}
                                    }
                                    app.settings_buf.clear();
                                }
                                KeyCode::Backspace => { app.settings_buf.pop(); }
                                KeyCode::Char(c) => { app.settings_buf.push(c); }
                                _ => {}
                            }
                            continue;
                        }

                        match key.code {
                            // ---- global ----
                            KeyCode::Char('q') => {
                                kill_mpv(&mut app.mpv_process);
                                return Ok(());
                            }
                            KeyCode::Char('?') => { app.show_help = !app.show_help; }
                            KeyCode::Esc => {
                                if app.show_help { app.show_help = false; }
                                else if app.view != AppView::NowPlaying {
                                    app.view = AppView::NowPlaying;
                                }
                            }

                            // ---- F-key tabs ----
                            KeyCode::F(1) => { app.view = AppView::NowPlaying; }
                            KeyCode::F(2) => {
                                app.queue_cursor = app.jukebox_index.max(0) as usize;
                                app.view = AppView::Queue;
                            }
                            KeyCode::F(3) | KeyCode::Char('p') => {
                                if app.playlists.is_empty() {
                                    match get_playlists(&app.config) {
                                        Ok(pls) => app.playlists = pls,
                                        Err(e) => error!("Playlists: {}", e),
                                    }
                                }
                                app.view = AppView::Playlists;
                            }
                            KeyCode::F(4) => {
                                app.sync_settings_from_config();
                                app.settings_cursor = 0;
                                app.view = AppView::Settings;
                            }

                            // ---- playback ----
                            KeyCode::Char(' ') => {
                                if app.jukebox_playing {
                                    mpv_ipc("{\"command\":[\"set_property\",\"pause\",true]}\n");
                                    app.jukebox_playing = false;
                                } else {
                                    mpv_ipc("{\"command\":[\"set_property\",\"pause\",false]}\n");
                                    app.jukebox_playing = true;
                                }
                            }
                            KeyCode::Char(']') => {
                                let idx = (app.jukebox_index + 1) as usize;
                                if idx < app.tracks.len() { play_track(&mut app, idx); }
                            }
                            KeyCode::Char('[') => {
                                let idx = (app.jukebox_index - 1).max(0) as usize;
                                play_track(&mut app, idx);
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                let gain = (app.jukebox_gain + 0.1).min(1.0);
                                mpv_ipc(&format!("{{\"command\":[\"set_property\",\"volume\",{}]}}\n", (gain * 100.0).round() as u32));
                                app.jukebox_gain = gain;
                            }
                            KeyCode::Char('-') => {
                                let gain = (app.jukebox_gain - 0.1).max(0.0);
                                mpv_ipc(&format!("{{\"command\":[\"set_property\",\"volume\",{}]}}\n", (gain * 100.0).round() as u32));
                                app.jukebox_gain = gain;
                            }
                            KeyCode::Char('l') => {
                                app.loop_mode = app.loop_mode.next();
                                app.status = format!("Loop: {}", app.loop_mode.label());
                            }

                            // ---- navigation (view-specific) ----
                            KeyCode::Down | KeyCode::Char('j') => {
                                match app.view {
                                    AppView::NowPlaying => { app.scroll += 1; }
                                    AppView::Queue => {
                                        if app.queue_cursor + 1 < app.tracks.len() {
                                            app.queue_cursor += 1;
                                        }
                                    }
                                    AppView::Playlists => match app.playlist_focus {
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
                                    },
                                    AppView::Settings => {
                                        if app.settings_cursor < 3 { app.settings_cursor += 1; }
                                    }
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                match app.view {
                                    AppView::NowPlaying => { app.scroll = app.scroll.saturating_sub(1); }
                                    AppView::Queue => { app.queue_cursor = app.queue_cursor.saturating_sub(1); }
                                    AppView::Playlists => match app.playlist_focus {
                                        PlaylistFocus::Playlists => { app.playlist_cursor = app.playlist_cursor.saturating_sub(1); }
                                        PlaylistFocus::Tracks => { app.track_cursor = app.track_cursor.saturating_sub(1); }
                                    },
                                    AppView::Settings => { app.settings_cursor = app.settings_cursor.saturating_sub(1); }
                                }
                            }

                            KeyCode::Tab => {
                                if app.view == AppView::Playlists {
                                    app.playlist_focus = match app.playlist_focus {
                                        PlaylistFocus::Playlists => PlaylistFocus::Tracks,
                                        PlaylistFocus::Tracks => PlaylistFocus::Playlists,
                                    };
                                }
                            }

                            KeyCode::Enter => {
                                match app.view {
                                    AppView::Queue => {
                                        let idx = app.queue_cursor;
                                        play_track(&mut app, idx);
                                        app.view = AppView::NowPlaying;
                                    }
                                    AppView::Playlists => match app.playlist_focus {
                                        PlaylistFocus::Playlists => {
                                            if let Some(pl) = app.playlists.get(app.playlist_cursor) {
                                                match get_playlist_tracks(&app.config, &pl.id.clone()) {
                                                    Ok(tracks) => {
                                                        app.tracks = tracks;
                                                        app.track_cursor = 0;
                                                        app.playlist_focus = PlaylistFocus::Tracks;
                                                    }
                                                    Err(e) => error!("Tracks: {}", e),
                                                }
                                            }
                                        }
                                        PlaylistFocus::Tracks => {
                                            let idx = app.track_cursor;
                                            play_track(&mut app, idx);
                                            app.view = AppView::NowPlaying;
                                        }
                                    },
                                    AppView::Settings => {
                                        match app.settings_cursor {
                                            0 | 1 | 2 => {
                                                // start editing text field
                                                app.settings_editing = true;
                                                app.settings_buf = match app.settings_cursor {
                                                    0 => app.settings_url.clone(),
                                                    1 => app.settings_user.clone(),
                                                    2 => app.settings_refresh.clone(),
                                                    _ => String::new(),
                                                };
                                            }
                                            3 => { app.settings_karaoke = !app.settings_karaoke; }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            KeyCode::Char('s') if app.view == AppView::Settings => {
                                app.save_settings();
                            }

                            _ => {}
                        }
                    }
                }
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
// UI routing
// ----------------------------------------
fn ui(f: &mut Frame, app: &AppState) {
    let area = f.area();

    // Tab bar (1 line) + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_tab_bar(f, chunks[0], app);

    match app.view {
        AppView::NowPlaying => {
            let layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
                .split(chunks[1]);
            render_left(f, layout[0], app);
            f.render_widget(render_lyrics(app), layout[1]);
        }
        AppView::Queue => render_queue(f, chunks[1], app),
        AppView::Playlists => render_playlists(f, chunks[1], app),
        AppView::Settings => render_settings(f, chunks[1], app),
    }

    if app.show_help { render_help(f); }
}

fn render_tab_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let tabs = [
        (AppView::NowPlaying, "F1 Now Playing"),
        (AppView::Queue,      "F2 Queue"),
        (AppView::Playlists,  "F3 Playlists"),
        (AppView::Settings,   "F4 Settings"),
    ];

    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (view, label) in &tabs {
        let active = app.view == *view;
        let style = if active {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} ", label), style));
        spans.push(Span::styled("  ", Style::default()));
    }

    // Loop indicator
    if app.loop_mode != LoopMode::Off {
        spans.push(Span::styled(
            format!("↻ {}", app.loop_mode.label()),
            Style::default().fg(Color::Cyan),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----------------------------------------
// Now Playing — left pane
// ----------------------------------------
fn render_left(f: &mut Frame, area: Rect, app: &AppState) {
    // Split: cover art rows at top, track info below
    let art_height = if app.is_kitty {
        if app.cover_art_kitty.is_some() { 10 } else { 0 }
    } else if app.cover_art_lines.is_empty() {
        0
    } else {
        app.cover_art_lines.len() as u16
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(art_height), Constraint::Min(0)])
        .split(area);

    // Cover art — halfblock for non-Kitty; Kitty draws after terminal.draw() via kitty_draw()
    if app.is_kitty {
        if app.cover_art_kitty.is_some() {
            // Blank placeholder so Ratatui doesn't put characters over the image area
            f.render_widget(Block::default(), chunks[0]);
        }
    } else if !app.cover_art_lines.is_empty() {
        f.render_widget(Paragraph::new(app.cover_art_lines.clone()), chunks[0]);
    }

    // Track info
    let info_area = if art_height > 0 { chunks[1] } else { area };
    let mut lines: Vec<Line<'static>> = vec![];

    if !app.title.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Title:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(app.title.clone()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Artist: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(app.artist.clone()),
        ]));
        if !app.album.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Album:  ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(app.album.clone()),
            ]));
        }

        if app.duration_seconds > 0 {
            let w = 20usize;
            let filled = (app.progress * w as f32).round() as usize;
            let bar = format!("{}{}", "█".repeat(filled.min(w)), "░".repeat(w - filled.min(w)));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!("[{}] {:02}:{:02} / {:02}:{:02}", bar,
                    app.progress_seconds / 60, app.progress_seconds % 60,
                    app.duration_seconds / 60, app.duration_seconds % 60),
                Style::default().fg(Color::Green),
            )));
        }

        {
            let w = 10usize;
            let filled = (app.jukebox_gain * w as f32).round() as usize;
            let bar = format!("{}{}", "█".repeat(filled.min(w)), "░".repeat(w - filled.min(w)));
            let icon = if app.jukebox_playing { "▶" } else { "⏸" };
            lines.push(Line::from(Span::styled(
                format!("{} Vol [{}] {:3.0}%", icon, bar, app.jukebox_gain * 100.0),
                Style::default().fg(Color::Cyan),
            )));
        }

        if let Some(ref w) = app.current_word {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!("♪ {} ♪", w),
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            )));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(app.status.clone(), Style::default().fg(Color::Yellow))));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title("Track Info")
                .border_style(Style::default().fg(Color::Green))
        ),
        info_area,
    );
}

// ----------------------------------------
// Lyrics pane
// ----------------------------------------
fn render_lyrics(app: &AppState) -> Paragraph<'static> {
    let current = app.current_line as usize;

    let lines: Vec<Line<'static>> = if !app.synced.is_empty() {
        app.synced.iter().enumerate().map(|(i, sl)| {
            let is_cur = i == current;
            if is_cur && app.config.karaoke_enabled {
                if let Some(ref cw) = app.current_word {
                    let text = sl.text.clone();
                    if let Some(pos) = text.find(cw.as_str()) {
                        let before = text[..pos].to_string();
                        let word   = text[pos..pos + cw.len()].to_string();
                        let after  = text[pos + cw.len()..].to_string();
                        return Line::from(vec![
                            Span::styled(before, Style::default().fg(Color::DarkGray)),
                            Span::styled(word, Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                            Span::styled(after, Style::default().fg(Color::DarkGray)),
                        ]);
                    }
                }
                Line::from(Span::styled(sl.text.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if is_cur {
                Line::from(Span::styled(sl.text.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if i > current && i <= current + 3 {
                Line::from(Span::styled(sl.text.clone(), Style::default().fg(Color::Gray)))
            } else {
                Line::from(Span::styled(sl.text.clone(), Style::default().fg(Color::DarkGray)))
            }
        }).collect()
    } else {
        app.cached_lines.clone()
    };

    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Lyrics")
            .border_style(Style::default().fg(Color::Blue)))
        .scroll((app.scroll, 0))
        .wrap(Wrap { trim: false })
}

// ----------------------------------------
// Queue view (F2)
// ----------------------------------------
fn render_queue(f: &mut Frame, area: Rect, app: &AppState) {
    let lines: Vec<Line> = if app.tracks.is_empty() {
        vec![Line::from(Span::styled(
            "No queue — open F3 Playlists and press Enter on a track.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.tracks.iter().enumerate().map(|(i, t)| {
            let mins = t.duration / 60;
            let secs = t.duration % 60;
            let is_playing = i == app.jukebox_index as usize && app.mpv_process.is_some();
            let icon = if is_playing { "▶ " } else { "  " };
            let label = format!("{}{:2}. {} — {} ({:02}:{:02})", icon, i + 1, t.title, t.artist, mins, secs);

            if i == app.queue_cursor {
                Line::from(Span::styled(label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if is_playing {
                Line::from(Span::styled(label, Style::default().fg(Color::Green)))
            } else {
                Line::from(Span::raw(label))
            }
        }).collect()
    };

    let scroll = if app.queue_cursor > 5 { (app.queue_cursor - 5) as u16 } else { 0 };

    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Queue  [Enter=play  [/]=prev/next]")
                .border_style(Style::default().fg(Color::Cyan)))
            .scroll((scroll, 0)),
        area,
    );
}

// ----------------------------------------
// Playlists view (F3)
// ----------------------------------------
fn render_playlists(f: &mut Frame, area: Rect, app: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

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

    f.render_widget(
        Paragraph::new(pl_lines)
            .block(Block::default().borders(Borders::ALL).title("Playlists")
                .border_style(if pl_focus { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }))
            .wrap(Wrap { trim: false }),
        layout[0],
    );

    let tr_focus = app.playlist_focus == PlaylistFocus::Tracks;
    let tr_lines: Vec<Line> = if app.tracks.is_empty() {
        vec![Line::from(Span::styled("Select a playlist → Enter", Style::default().fg(Color::DarkGray)))]
    } else {
        app.tracks.iter().enumerate().map(|(i, t)| {
            let mins = t.duration / 60;
            let secs = t.duration % 60;
            let is_playing = i == app.jukebox_index as usize && app.mpv_process.is_some();
            let label = format!(" {}{} — {} ({:02}:{:02})", if is_playing { "▶ " } else { "" }, t.title, t.artist, mins, secs);
            if i == app.track_cursor && tr_focus {
                Line::from(Span::styled(label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
            } else if i == app.track_cursor {
                Line::from(Span::styled(label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            } else if is_playing {
                Line::from(Span::styled(label, Style::default().fg(Color::Green)))
            } else {
                Line::from(Span::raw(label))
            }
        }).collect()
    };

    let tr_scroll = if tr_focus && app.track_cursor > 5 { (app.track_cursor - 5) as u16 } else { 0 };
    let track_title = app.playlists.get(app.playlist_cursor)
        .map(|pl| format!("Tracks — {}", pl.name))
        .unwrap_or_else(|| "Tracks".to_string());

    f.render_widget(
        Paragraph::new(tr_lines)
            .block(Block::default().borders(Borders::ALL).title(track_title)
                .border_style(if tr_focus { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }))
            .scroll((tr_scroll, 0))
            .wrap(Wrap { trim: false }),
        layout[1],
    );
}

// ----------------------------------------
// Settings view (F4)
// ----------------------------------------
fn render_settings(f: &mut Frame, area: Rect, app: &AppState) {
    let fields: &[(&str, String, bool)] = &[
        ("Server URL",        app.settings_url.clone(),     true),
        ("Username",          app.settings_user.clone(),    true),
        ("Refresh Interval",  app.settings_refresh.clone(), true),
        ("Karaoke Mode",      if app.settings_karaoke { "Enabled".into() } else { "Disabled".into() }, false),
    ];

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled("  Settings  (s=save  Enter=edit  Space=toggle  Esc=cancel)", Style::default().fg(Color::DarkGray))),
        Line::default(),
    ];

    for (i, (label, value, editable)) in fields.iter().enumerate() {
        let selected = i == app.settings_cursor;
        let is_editing = selected && app.settings_editing;

        let display_val = if is_editing {
            format!("{}█", app.settings_buf)    // cursor indicator
        } else {
            value.clone()
        };

        let edit_hint = if *editable { "  [Enter]" } else { "  [Space]" };

        let line = if selected {
            Line::from(vec![
                Span::styled(
                    format!("  ▶ {:18} ", label),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    display_val,
                    if is_editing {
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    },
                ),
                Span::styled(edit_hint, Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::styled(format!("    {:18} ", label), Style::default().fg(Color::Gray)),
                Span::styled(display_val, Style::default().fg(Color::White)),
            ])
        };

        lines.push(line);
        lines.push(Line::default());
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "    Token/Salt: managed automatically — re-run setup to change credentials",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title("Settings")
                .border_style(Style::default().fg(Color::Magenta))
        ),
        area,
    );
}

// ----------------------------------------
// Help overlay
// ----------------------------------------
fn render_help(f: &mut Frame) {
    let popup = centered_rect(50, 20, f.area());
    let help = vec![
        Line::from(Span::styled(" Views", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  F1          Now Playing"),
        Line::from("  F2          Queue"),
        Line::from("  F3 / p      Playlists"),
        Line::from("  F4          Settings"),
        Line::default(),
        Line::from(Span::styled(" Playback", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  Space       Play / Pause"),
        Line::from("  ] / [       Next / Previous"),
        Line::from("  + / -       Volume"),
        Line::from("  l           Cycle loop mode"),
        Line::default(),
        Line::from(Span::styled(" Navigation", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("  j / k ↑↓    Scroll / navigate"),
        Line::from("  Tab         Switch pane (playlists)"),
        Line::from("  Enter       Select / play / edit"),
        Line::from("  q           Quit"),
        Line::from("  ? / Esc     Close help"),
    ];
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(help).block(
            Block::default().borders(Borders::ALL).title(" Help ")
                .border_style(Style::default().fg(Color::Cyan))
        ),
        popup,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}
