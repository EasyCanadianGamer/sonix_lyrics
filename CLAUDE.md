# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                  # debug build
cargo build --release        # release build
cargo run                    # run (config wizard fires on first run if no config)
cargo clippy                 # lint
cargo fmt                    # format
./scripts/build-release.sh   # strip + copy release binary to dist/
```

There are no automated tests.

## Architecture

Sonix Lyrics is a terminal app (Ratatui + Crossterm) that polls a Navidrome server for the currently playing track and displays synced lyrics fetched from lrclib.net.

### Threading model

Three concurrent threads communicate via crossbeam bounded channels:
- **Metadata thread** — calls `navidrome::get_current_track()` on a configurable interval (default 2s), sends `Track` updates to main
- **Tick thread** — sends 100ms ticks to drive UI redraws (~30fps with 33ms throttle in the render path)
- **Main thread** — event loop that processes metadata, ticks, and crossterm keyboard events

### Modules

| File | Role |
|------|------|
| `src/main.rs` | `AppState`, event loop, playback clock, TUI rendering (35/65 split pane) |
| `src/navidrome.rs` | Subsonic API client — `get_current_track()` calls `getNowPlaying`, deserialises JSON |
| `src/lyrics.rs` | LRC parser, lrclib.net search, karaoke word-timing extraction |
| `src/config.rs` | Key=value config file at `~/.config/sonix_lyrics/config.conf` |
| `src/setup.rs` | First-run TUI wizard; generates MD5 token from `md5(password + salt)` |

### Playback clock

Elapsed time is derived from Navidrome's `played_timestamp` (UTC) field:
`elapsed_ms = (Utc::now() - start_timestamp_utc).num_milliseconds()`

The smooth playback timer is intentionally disabled — it caused drift. Do not re-enable without fixing the drift issue.

### Karaoke

`KARAOKE_ENABLED = false` by default and currently disabled in the UI pending stability fixes. The data structures (`KaraokeWord`, `SyncedLine.words`) and parsing (`parse_karaoke_words`) are complete; only the rendering/sync is broken.

### Navidrome auth

Uses Subsonic token auth: `t = md5(password + salt)`, `s = random 12-char lowercase salt`. Credentials are stored pre-computed in the config file (never the plaintext password).

### Known issues (alpha)
- Karaoke word highlighting is broken — `karaoke_enabled` guards all related rendering paths
- Smooth playback timer disabled due to drift
- Primarily tested on NixOS; untested on other distros
