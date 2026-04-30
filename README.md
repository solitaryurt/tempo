# Tempo

Local-first desktop music player and library manager built with Rust and [GPUI](https://www.gpui.rs/). Linux-first (Arch), with macOS and Windows builds tracked in CI. Inspired by Foobar2000's table-first density and keyboard workflow rather than streaming-app conventions.

## Features

- Recursive library scanning with live filesystem watching (`walkdir` + `notify`).
- Tag reading via `lofty`; primary-vs-credited artist normalization for grouping.
- Bundled SQLite catalog (`rusqlite` with `bundled` feature) for tracks, albums, artists, playlists, history, and search.
- Tabbed browsing: All Music, Albums, Artists, Playlists, Liked, History, plus per-artist/album detail tabs.
- Sortable table with multi-column layouts, row context menus, keyboard navigation.
- Audio playback through `rodio`; FFT analysis (`rustfft`) drives a frequency-bar and dancing-line visualizer.
- 10-band parametric equalizer with builtin presets and bypass.
- Analytics page: listening heatmaps, weekday/hour clock, genre/format/sample-rate breakdowns, library growth.
- Mini-player mode with multiple presentation styles.
- XDG-compliant state, cache, and database paths.

## Architecture

| Path | Responsibility |
|---|---|
| `src/main.rs` | GPUI actions, keybindings, window startup. |
| `src/app/` | UI layer: `mod.rs` owns `TempoApp` and tab state; sibling modules split rendering (`table`, `sidebar`, `player`, `library_view`, `browse_grids`, `analytics`, `equalizer_panel`, `settings`, `search`, `theme`, `artwork`, etc.). |
| `src/library.rs` | Filesystem scanning, watching, `LibraryEvent` emission. |
| `src/catalog.rs` | SQLite schema, migrations (in-code, no separate SQL files), browse queries, metadata helpers. |
| `src/playback.rs` | `rodio`-based playback wrapper. |
| `src/audio_analyzer.rs` | FFT + band aggregation for visualizers. |
| `src/equalizer.rs` | EQ DSP and preset handling. |
| `src/metadata_worker.rs` | Background tag/artwork extraction jobs. |
| `src/snapshot.rs` | App-state persistence. |

PRD documents in the repo (`PRD.md`, `ONLINE_METADATA_ENRICHMENT.md`) describe future direction (Symphonia/CPAL, online enrichment) that is **not** in the current code.

## Data Paths

- State: `$XDG_CONFIG_HOME/tempo/state.json` (fallback `~/.config/tempo/state.json`).
- Catalog DB: `$XDG_DATA_HOME/tempo/tempo.sqlite` (fallback `~/.local/share/tempo/tempo.sqlite`).
- Artwork cache: `$XDG_CACHE_HOME/tempo` (fallback `~/.cache/tempo`).
- Library roots: `TEMPO_MUSIC_DIR` env override → saved roots → `~/Music` if present.

## Building

Requires stable Rust (`edition = "2024"`).

### Linux

System packages (Arch / Debian names vary):

```
alsa-lib fontconfig libx11 libxcb libxkbcommon pkgconf
```

Debian/Ubuntu equivalents are in `.github/workflows/rust.yml`.

```sh
cargo build --release
./target/release/tempo
```

### macOS

```sh
cargo build --release
```

No extra system dependencies; Metal is provided by the OS. Apple Silicon is the default CI target.

### Windows

GPUI's Windows backend is still maturing; the CI job is `continue-on-error`. Build locally with the MSVC toolchain:

```powershell
cargo build --release
```

Expect breakage until upstream GPUI Windows support stabilizes.

## Development

The repo uses an `rtk` cargo wrapper (see `AGENTS.md`):

```sh
rtk cargo fmt
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk cargo run
```

Tests use temp dirs and require no external services. Focus a single test with `rtk cargo test <name>`.

## CI and Releases

`.github/workflows/rust.yml` runs fmt + clippy + test + release build on Linux, plus build-only jobs on macOS and Windows. Each successful job uploads its binary as a workflow artifact. Pushing a `v*` tag also creates a GitHub Release with the binaries attached.

## Keybindings

Playback: `Enter` play selected, `Space` toggle pause, `Left`/`Right` move selection, `Ctrl+R` random track.
Tabs: `Ctrl+T` new, `Ctrl+W` close, `Ctrl+Shift+T` reopen, `Ctrl+Tab`/`Ctrl+Shift+Tab` cycle, `Ctrl+1..9,0` jump.
Navigation: `Alt+Left`/`Alt+Right` history, `Ctrl+F` or `/` focus search, `Ctrl+S` settings.
Player: `Ctrl+M` toggle mini-player, `Ctrl+Shift+M` cycle mini-player styles.

## Status

Pre-1.0. Schema, UI surfaces, and module boundaries change without notice. No tag writing, no MPRIS yet, no online metadata enrichment yet, no packaging.
