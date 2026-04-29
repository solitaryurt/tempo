# AGENTS.md

## Commands

- Use the repo's documented command wrapper: `rtk cargo fmt`, `rtk cargo check`, `rtk cargo test`, `rtk cargo run`.
- Focus one test by name with `rtk cargo test <test_name>`; tests currently use temp dirs and do not require external services.
- Run `rtk cargo fmt` before committing Rust changes; there is no repo-local CI, clippy config, pre-commit config, or task runner.

## Current Architecture

- `README.md` is partly stale: the app is no longer a single `src/main.rs` mock prototype.
- `src/main.rs` only wires GPUI actions, keybindings, and window startup; the UI lives under `src/app/`.
- `src/app/mod.rs` owns `TempoApp`, page/tab state, domain structs used by the UI, and module wiring.
- `src/app/library_view.rs`, `table.rs`, `browse_grids.rs`, `sidebar.rs`, `player.rs`, `settings.rs`, `search.rs`, `artwork.rs`, and `theme.rs` split GPUI rendering and interaction code.
- `src/library.rs` scans/watches local files with `walkdir`, `notify`, and `lofty`; it emits `LibraryEvent`s and can write through `CatalogStore`.
- `src/catalog.rs` owns the bundled-SQLite schema/migrations, cache paths, browse queries, metadata job helpers, and catalog tests.
- `src/playback.rs` is the current `rodio` playback wrapper; PRD references Symphonia/CPAL are future direction, not current code.

## State And Data Paths

- App state is JSON at `$XDG_CONFIG_HOME/tempo/state.json` or `~/.config/tempo/state.json`.
- The catalog DB is `$XDG_DATA_HOME/tempo/tempo.sqlite` or `~/.local/share/tempo/tempo.sqlite`.
- Artwork/cache files are under `$XDG_CACHE_HOME/tempo` or `~/.cache/tempo`.
- `TEMPO_MUSIC_DIR` overrides saved/default library roots; otherwise the app uses saved roots, then `~/Music` if it exists.

## Repo-Specific Gotchas

- GPUI `.on_click(...)` requires a stateful element; add a stable `.id(...)` to new clickable `div()`s.
- Browse tabs are not just pages: `TabSource` includes All Music, playlists, artist detail, and album detail tabs. Detail navigation should open/reuse tabs rather than overwriting All Music searches.
- Track display preserves raw artist credits, but artist/album grouping normalizes featured credits in `catalog.rs` via `primary_artist_name` / `individual_artist_names`; keep profile filtering in `src/app/search.rs` aligned with that behavior.
- `CatalogStore::migrate()` is the migration source of truth; there are no separate SQL migration files.
- Online metadata enrichment is design-only for now. `ONLINE_METADATA_ENRICHMENT.md` documents intended APIs/jobs, but network workers/settings are intentionally not implemented.
