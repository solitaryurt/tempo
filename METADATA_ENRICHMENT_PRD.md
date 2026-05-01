# Tempo Metadata Enrichment PRD

Multi-source online metadata enrichment, album descriptions, robust API
error handling, full artist discographies (including missing albums), and
a unified Errors page.

This document is the build-time spec. It supersedes the
forward-looking sections of `ONLINE_METADATA_ENRICHMENT.md` and folds in
the newer requirements that landed during planning.

## Goals

1. Surface artist bios on the artist single view *and* album descriptions
   on the album single view, populated from external sources, cached in
   SQLite, and rendered without blocking the UI.
2. Make every API call's failure mode legible: log non-2xx responses
   with status, URL, and body excerpt; classify errors so the UI can
   filter them.
3. Treat Discogs as a first-class enrichment source (no API key needed,
   25 req/min ceiling) and use it specifically to populate full artist
   discographies, including releases the user does not own. Missing
   items render greyed out next to local ones.
4. Replace the existing "Scan Errors" page with a unified "Errors"
   page that shows scan failures and metadata-job failures together,
   with toggleable badge filters at the top.
5. Keep Tempo local-first: enrichment stays opt-in via the existing
   `OnlineMetadataMode` setting (`Off` | `Automatic`), defaulting to
   `Off`.

## Non-goals

- Online playback, streaming providers, or any cloud-stored library.
- Editing metadata back to source providers.
- Logging API errors to a separate file. `perf::event` plus the
  persisted `metadata_jobs.last_error` and `metadata_*.metadata_error`
  columns are the only logging surfaces.
- Persisting Errors-page filter selections across launches. Filter
  state is runtime-only and resets to all-on each launch.
- Adding any user-facing credential / API-key fields. Discogs is used
  unauthenticated.

## Current state (baseline)

These are the relevant pieces already in tree at the start of build:

- `OnlineMetadataMode { Off, Automatic }` defined in `src/app/mod.rs`
  and surfaced through `SettingsSection::OnlineMetadata` rendered by
  `render_online_metadata_settings` in `src/app/settings.rs`.
- `MetadataWorker` in `src/metadata_worker.rs` with five job types:
  `resolve_artist_musicbrainz`, `fetch_artist_profile`,
  `fetch_artist_discography`, `resolve_album_musicbrainz`,
  `fetch_album_cover`.
- `start_metadata_event_loop` in `src/app/library_state.rs` already
  reloads catalog browse data on `MetadataEvent::ArtistUpdated` /
  `AlbumUpdated`. Artist bios appear in the UI as soon as they are
  written to the catalog -- but only when Online Metadata is set to
  `Automatic`.
- `start_metadata_activity_poll` in `src/app/library_state.rs` polls
  `CatalogMetadataActivity` every `METADATA_ACTIVITY_TICK` while mode is
  `Automatic`. This drives the metadata-sync status pill.
- `Page::ScanErrors` in `src/app/mod.rs`, rendered by
  `render_scan_errors_page` in `src/app/library_view.rs`. Backed by
  `scan_errors: Vec<IndexingError>` populated from
  `LibraryEvent::ScanError`.
- Album detail hero (`render_album_detail_hero` in
  `src/app/browse_grids.rs`) currently substitutes the *artist's* bio
  for the album description, with a synthetic local fallback. This is
  removed in this work.
- `discography_items` table in `src/catalog.rs` already exists, keyed
  on `(artist_id, normalized_title, release_type)`, with MB
  release-group identification. UI is wired through the artist detail
  hero but only ever sees MB-sourced rows.

## High-level deliverables

1. Schema v5 with new fields for descriptions, error classification,
   and Discogs identifiers.
2. `request_json` / `request_bytes` HTTP helpers in the metadata worker
   that capture and persist non-2xx response context.
3. Album-description primary path via TheAudioDB.
4. Multi-source fallback chains for artist identity, artist bio,
   album identity, album description, album cover, and full
   discography.
5. Discogs integration (unauthenticated, 25 req/min throttle), with
   `/artists/{id}/releases` powering full discography enrichment.
   Per-row thumb downloads happen as separate per-item jobs to keep the
   rate limiter honest.
6. Unified Errors page replacing Scan Errors, with badge-style filters.

## Phases

### Phase 0: schema and HTTP plumbing

- Bump `SCHEMA_VERSION` 4 -> 5 in `src/catalog.rs` with a v4 -> v5
  rationale comment in the same style as the existing v3 -> v4 note.
- Idempotent additions inside `migrate()`:
  - `albums.description TEXT`
  - `albums.description_source TEXT`
  - `albums.discogs_master_id TEXT` (with unique index)
  - `artists.discogs_id TEXT`
  - `metadata_jobs.error_kind TEXT`
  - `metadata_jobs.tried_sources TEXT`
  - `discography_items.discogs_master_id TEXT` (with unique index)
  - `discography_items.source TEXT NOT NULL DEFAULT 'musicbrainz'`
  - `discography_items.role TEXT NOT NULL DEFAULT 'Main'`
  - `discography_items.format TEXT`
  - Drop and recreate the `discography_items` unique index as
    `UNIQUE(artist_id, normalized_title, release_type, role)`.
- Add `request_json::<T>` and `request_bytes` helpers in
  `src/metadata_worker.rs`. On non-2xx these capture status, final URL,
  `Retry-After` header, and up to 512 bytes of body, then emit
  `perf::event("metadata.api.http_error", "source=... status=... url=... body=...")`
  and return a `MetadataApiError` carrying an `error_kind` enum
  (`Network`, `Http4xx`, `Http5xx`, `Parse`, `NoMatch`).
- The helpers always strip `Authorization` from any captured request
  context (defense in depth; no Discogs token today, but the redaction
  is unconditional).
- Refactor existing call sites in `metadata_worker.rs`
  (`resolve_artist_musicbrainz`, `fetch_artist_profile`,
  `fetch_artist_discography`, `resolve_album_musicbrainz`,
  `fetch_album_cover`, `download_external_asset`) to use the helpers.
  Cover Art Archive 404 stays a `NoMatch`/"missing" outcome, not an
  error.
- Extend `fail_metadata_job` and `mark_*_metadata_checked` to also
  persist `error_kind` and append the source name to `tried_sources`.

Verification: `rtk cargo check`, `rtk cargo test`, manual confirmation
that an existing install upgrades cleanly (one-shot migration; all
operations idempotent).

### Phase 1: album descriptions (TheAudioDB primary)

- New job constant `FETCH_ALBUM_PROFILE = "fetch_album_profile"` plus
  dispatch in `run_worker`.
- New `fetch_album_profile` calls
  `https://www.theaudiodb.com/api/v1/json/123/album-mb.php?i=<rg_mbid>`,
  parses `strDescriptionEN`, persists via new
  `catalog::save_album_profile(album_id, audiodb_id, description)`.
- `CatalogAlbum` gains `description: Option<String>` populated by
  `load_albums` (use `MAX(albums.description)` so the existing GROUP BY
  on `(albums.id, tracks.artist_name)` still works).
- `load_metadata_album` returns description-state fields needed for
  fallback decisions (`metadata_status`, `error_kind`).
- Backfill via `enqueue_missing_online_metadata_jobs`. On-demand via
  new `enqueue_album_profile_demand`.
- `CatalogMetadataActivity` gains `pending_album_profile`. Update
  `metadata_sync_eta_label` weights.
- `Album` struct in `src/app/mod.rs` gains
  `description: Option<String>` and
  `description_state: AlbumDescriptionState { Pending, Available, Unavailable }`.
- `From<CatalogAlbum> for Album` populates both fields.
- `album_searchable_lower` includes the description.
- New `queue_album_profile_demand(album_id)` in `src/app/mod.rs`,
  parallel to `queue_album_cover_demand`, gated on
  `OnlineMetadataMode::Automatic`.
- Snapshot codec (`src/snapshot.rs`): bump version, add
  `Option<String>` description after `artwork_path` in
  `write_album` / `read_album`.
- `render_album_detail_hero` in `src/app/browse_grids.rs`:
  - Use `album.description` when present.
  - Otherwise call `queue_album_profile_demand` and render the
    existing synthetic local fallback.
  - When `description_state == Unavailable`, render
    `"No online description available."` instead of the synthetic
    sentence.
  - Remove the artist-bio-as-album-description hack.

Verification: `rtk cargo check`, `rtk cargo test`, plus a new unit
test for `save_album_profile` round-trip in `catalog.rs` and a new
snapshot round-trip test covering `Album.description = Some(...)`.

### Phase 2: TheAudioDB and Wikipedia fallbacks (artist + album)

New job types and wiring (all unconditional except the existing
`OnlineMetadataMode::Automatic` gate):

- `resolve_artist_audiodb_search` -- TheAudioDB `/search.php?s=<name>`
  -> adopt `strMusicBrainzID` if present, then re-enqueue the standard
  MB-based jobs. Enqueued by `resolve_artist_musicbrainz` on no MB
  match (instead of going straight to `missing`).
- `resolve_album_audiodb_search` -- TheAudioDB
  `/searchalbum.php?s=<artist>&a=<album>` -> adopt release-group MBID.
  Enqueued by `resolve_album_musicbrainz` on no match.
- `fetch_artist_wikipedia_summary` -- uses MusicBrainz artist lookup
  with `inc=url-rels`, picks the Wikipedia rel preferring
  `en.wikipedia.org`, calls
  `https://en.wikipedia.org/api/rest_v1/page/summary/<title>`, stores
  the full `extract` as bio. Enqueued when `fetch_artist_profile`
  returns no `strBiographyEN`.
- `fetch_album_wikipedia_summary` -- same pattern via release-group
  url-rels. Enqueued when `fetch_album_profile` returns no description.

Rate limiting:

- Reuse existing `MUSICBRAINZ_DELAY` (1s) and `THEAUDIODB_DELAY` (2s).
- New `WIKIPEDIA_DELAY = Duration::from_millis(250)` with its own
  `last_wikipedia_request: Option<Instant>` slot in `run_worker`.

`tried_sources` accumulates across the chain. Only when all viable
sources fail does the entity move to `metadata_status = 'missing'`
with `error_kind = 'no_match'`.

Verification: new unit test exercising the source-priority fallback
ordering with mocked sources, plus a `tried_sources` accumulation
test.

### Phase 3: Discogs (unauthenticated)

Discogs is added without any user-facing settings. No token field, no
masked input, no setter. The reference for the artist-releases
endpoint is the Discogs Database API documentation
(https://www.discogs.com/developers/#page:database,header:database-artist-releases).

Worker plumbing:

- New constant `DISCOGS_DELAY = Duration::from_millis(2_400)` (60_000ms
  / 25 req = 2.4s minimum spacing). Add
  `last_discogs_request: Option<Instant>` slot in `run_worker`.
- Honor response headers when present:
  - `X-Discogs-Ratelimit-Remaining <= 1` forces the next request to
    wait the rest of the current window via `Retry-After` if provided,
    else 60 seconds.
  - `Retry-After` already handled by `request_json` for 429 / 5xx.
- `User-Agent` already meets Discogs's requirement; no other headers.

New job types:

- `resolve_artist_discogs_search` -- last-resort artist identity via
  `/database/search?type=artist&q=<name>`. Adopts `id` into
  `artists.discogs_id` and enqueues
  `fetch_artist_discogs_profile` and `fetch_artist_discogs_releases`.
  Enqueued by `resolve_artist_audiodb_search` on no match.
- `fetch_artist_discogs_profile` -- `GET /artists/<discogs_id>` for
  `profile` (used as bio fallback when Wikipedia/TheAudioDB came up
  empty) and `images[0].uri` (used as photo fallback). Tags
  `bio_source = 'discogs'` when adopted.
- `fetch_artist_discogs_releases` -- the discography-completeness job
  (see below).
- `resolve_album_discogs_search` -- last-resort album identity via
  `/database/search?type=master&artist=<a>&release_title=<t>`. Stores
  `master_id` in `albums.discogs_master_id`.
- `fetch_album_discogs_image` -- pulls the master/release primary
  image when CAA and TheAudioDB album thumb are both absent.
- `fetch_discogs_thumb` -- per-discography-item small-image GET via
  the existing `download_external_asset` path; persists
  `cover_asset_id` on the row. Enqueued one-per-row by
  `fetch_artist_discogs_releases`.

`fetch_artist_discogs_releases` design:

- Calls `GET /artists/{discogs_id}/releases?sort=year&sort_order=asc&per_page=100&page=N`.
- Paginates up to `MUSICBRAINZ_RELEASE_GROUP_MAX_PAGES` (5) pages
  total, mirroring the MB job.
- Ingests **all roles** (Main, Appearance, Composed By, Producer,
  etc.). Discogs's `role` is stored verbatim in
  `discography_items.role`. UI filtering decides what to show.
- Calls extended `catalog.upsert_discography_item` per item with the
  new arguments (role, format, source, discogs_master_id).
- Does not download thumbs inline. For each item with a `thumb` URL,
  enqueues a `fetch_discogs_thumb` job. The per-thumb jobs ride the
  same 2.4s Discogs throttle and are interleaved with other work and
  shutdown signals.

Discography priority chain:

- MB release-groups + Discogs `/artists/<id>/releases` populate the
  same `discography_items` table in parallel.
- Dedup is enforced by the new compound unique key
  `(artist_id, normalized_title, release_type, role)`.
- `upsert_discography_item` merges fields on conflict: keep MBID if
  present, set Discogs master id, prefer earliest known year, prefer
  the first cover asset that was downloaded.

Source priority chains (final):

| Goal                          | Sources (in order)                                                                                         |
|-------------------------------|------------------------------------------------------------------------------------------------------------|
| Artist identity               | MusicBrainz search -> TheAudioDB `/search.php` -> Discogs `/database/search?type=artist`                   |
| Artist bio + photo            | TheAudioDB `artist-mb.php` -> Wikipedia summary via MB url-rels -> Discogs `/artists/<id>`                 |
| Artist discography            | MusicBrainz release-groups + Discogs `/artists/<id>/releases` (parallel, dedup on merge)                   |
| Album release-group identity  | MusicBrainz release-group search -> TheAudioDB `searchalbum.php` -> Discogs `/database/search?type=master` |
| Album description             | TheAudioDB `album-mb.php` -> Wikipedia summary via release-group url-rels                                  |
| Album cover                   | Cover Art Archive -> TheAudioDB `strAlbumThumb` -> Discogs primary image                                   |

Discogs is intentionally not in the album-description chain (notes
fields are mostly tracklist annotations, not bios).

`CatalogMetadataActivity` adds:

- `pending_artist_discogs_releases`
- `pending_thumb_fetch`

`metadata_sync_eta_label` weights all new counters.

UI:

- Artist detail hero (`browse_grids.rs:19-106`) gains a small
  role-filter strip above the discography section. Default is "Main"
  visible. "Appearances" and "Other" toggleable.
- No source attribution shown to the user. `discography_items.source`
  stays internal.
- Existing UI behavior of greying out missing items (rows where
  `is_local = 0`) continues to work; Discogs simply contributes more
  rows to that set.

Verification: new unit tests:

- Discography dedup with both MB and Discogs sources for the same
  album.
- `fetch_artist_discogs_releases` ingests all roles and enqueues
  per-thumb jobs.
- 429 handling honors `Retry-After`.
- `tried_sources` accumulates across MB -> TheAudioDB -> Discogs.
- `upsert_discography_item` merge-on-conflict preserves earliest year
  and best cover.

### Phase 4: Errors page

All work stays in `src/app/library_view.rs` plus minimal touches to
the page enum and sidebar. No new module.

Renames:

- `Page::ScanErrors -> Page::Errors`
- `ScanErrorColumn -> ErrorColumn`
- `ScanErrorColumnWidths -> ErrorColumnWidths`
- `scan_errors_scroll_handle -> errors_scroll_handle`
- `render_scan_errors_* -> render_errors_*`

Persisted state alias: accept `"scan_errors"` on read for backward
compatibility; write `"errors"` going forward (`mod.rs:4456`).

`LibraryEvent::ScanError` and `IndexingError` are not renamed -- they
remain scan-domain types. The Errors page is the unification surface.

Sidebar:

- Label and icon updated to "Errors" (`sidebar.rs:217-218`, `:612`).

Page header:

- Subtitle: `"{N} errors from scans and online metadata"`.

Data:

- Keep `scan_errors: Vec<IndexingError>` as the named source for the
  `Scan` category.
- New `metadata_errors: Vec<MetadataErrorRow>` populated by new
  `catalog::load_metadata_errors()` (joins `metadata_jobs status='failed'`
  with `artists`/`albums` for context, surfacing path display
  strings like `"artist: Brian Eno"` or `"album: Kid A"`).
- Internal `ErrorView` enum wraps `&IndexingError | &MetadataErrorRow`
  for the table renderer.

Categories:

- `Scan`, `Network`, `Http4xx`, `Http5xx`, `Parse`, `NoMatch`. No
  `Blocked`; multi-source fallbacks eliminate that state.

Filtering:

- New `active_error_filters: BTreeSet<ErrorCategory>` on `TempoApp`,
  runtime-only, defaults to all-on each launch. Not persisted.
- New `render_errors_badge_bar` rendered between header and table.
  One pill per category showing label + count. Active pill uses
  `colors.accent`; inactive uses `colors.text_muted` / dimmed bg.
  Click toggles the category and `cx.notify()`s. Right side shows
  "All" / "None" shortcuts.
- `render_errors_table` adds a left-aligned "TYPE" column that renders
  the category badge inline. Filters out rows whose category is not in
  `active_error_filters`. Empty-state copy adapts based on whether
  filters are exclusionary.

Refresh:

- Piggyback on `start_metadata_activity_poll`. When mode is
  `Automatic` it now also calls `catalog.load_metadata_errors()` and
  stores into `app.metadata_errors`. Mode `Off` freezes errors at the
  last seen value (acceptable; user has disabled enrichment).
- Scan errors continue refreshing through `LibraryEvent::ScanError` /
  `ScanFinished` like today.

Verification: new unit tests for the persisted-state alias loading
old `"scan_errors"` page key, plus rendering tests if the codebase has
infrastructure for them.

## Cross-cutting items

### Logging

- `perf::event("metadata.api.http_error", ...)` on every non-2xx.
- `perf::event("metadata.api.parse_error", ...)` on JSON parse
  failures, including up to 256 bytes of body for triage.
- No file logging. The Errors page and the existing perf event stream
  are the only surfaces.

### Privacy and credentials

- Online enrichment defaults to `Off`.
- No user-supplied credentials. Discogs is unauthenticated.
- `request_json` strips `Authorization` headers from any captured
  request context unconditionally.

### Rate limits (final)

| Source           | Delay (min spacing) | Notes                                                 |
|------------------|---------------------|-------------------------------------------------------|
| MusicBrainz      | 1.0s                | Existing `MUSICBRAINZ_DELAY`.                         |
| TheAudioDB       | 2.0s                | Existing `THEAUDIODB_DELAY`.                          |
| Cover Art Archive| 0.25s               | Implicit via shared HTTP timeout.                     |
| Wikipedia REST   | 0.25s               | New `WIKIPEDIA_DELAY`.                                |
| Discogs          | 2.4s                | New `DISCOGS_DELAY`. Honor `X-Discogs-Ratelimit-*`.   |

### Snapshot format

`src/snapshot.rs` `SNAPSHOT_VERSION` bumps once. The change is
restricted to `write_album` / `read_album` adding an
`Option<String>` for description after `artwork_path`. The snapshot
is regenerated from SQLite on the next launch.

### Test additions (full list)

- `save_album_profile` round-trip in `catalog.rs`.
- Snapshot round-trip with `Album.description = Some(...)`.
- Source-priority fallback ordering with mocked sources.
- `tried_sources` accumulation after sequential failures.
- 429 handling honors `Retry-After`.
- `fetch_artist_discogs_releases` ingests all roles and enqueues
  per-thumb jobs.
- Discography dedup with both MB and Discogs sources for the same
  album.
- `upsert_discography_item` merge-on-conflict preserves earliest year
  and best cover.
- Persisted-state alias loads with old `"scan_errors"` page key.

## Risks

- **Eager-but-throttled thumb downloads** for deep Discogs catalogs
  can keep the activity-status pill busy for tens of minutes on first
  enrichment of a prolific artist. This is acceptable: the pill
  exists to communicate that work is in flight, and per-thumb jobs
  interleave cleanly with album covers and other work.
- **All-roles discography is noisy** without UI filtering. Default
  "Main only" filter is shipped from day one.
- **Schema v5 forces a one-shot migration re-run** on existing
  installs. All migration steps are idempotent.
- **Snapshot version bump invalidates the cached snapshot once.**
  First post-upgrade launch re-reads from SQLite.
- **Discogs unauthenticated** is shared per source IP at 25 req/min.
  `Retry-After` and `X-Discogs-Ratelimit-Remaining` handling protects
  against bursts.
- **`load_albums` GROUP BY** needs `MAX(albums.description)` and
  `MAX(albums.metadata_status)` for the new fields. Trivial.

## Implementation order (linear)

1. Phase 0: schema v5, `request_json` / `request_bytes`, refactor
   existing call sites, persist `error_kind` and `tried_sources`.
2. Phase 1: album descriptions (TheAudioDB primary), snapshot bump,
   `Album.description` + `description_state`, album hero UI.
3. Phase 2: TheAudioDB search fallbacks (artist + album) and
   Wikipedia summary fallbacks (artist + album).
4. Phase 3: Discogs jobs (artist identity, profile, releases,
   per-thumb), album identity + image fallbacks, role-filter strip on
   the artist hero.
5. Phase 4: Errors page rename, badge bar, multi-source rows,
   piggyback refresh on activity poll.
6. Final verification: `rtk cargo fmt`, `rtk cargo check`,
   `rtk cargo test`.
