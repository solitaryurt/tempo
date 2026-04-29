# Online Metadata Enrichment Research

## Goal

Tempo should enrich the local SQLite catalog with artist photos, artist bios, album art, external IDs, and full artist discographies while keeping playback, search, and library ingest local-first and responsive.

The enrichment layer should be optional, cached, rate-limited, and safe to retry.

## Recommended Sources

### MusicBrainz

Use MusicBrainz as the primary identity and discography source.

- API root: `https://musicbrainz.org/ws/2/`
- JSON support via `fmt=json` or `Accept: application/json`
- No API key required for read requests
- Requires a meaningful `User-Agent`
- Source IP limit is effectively 1 request per second
- Best uses: artist matching, artist MBIDs, release-group discographies, release group types, dates, official/release status metadata

Relevant endpoints:

- Search artist: `/ws/2/artist?query=<query>&fmt=json`
- Lookup artist: `/ws/2/artist/<mbid>?inc=aliases+genres+url-rels&fmt=json`
- Browse release groups by artist: `/ws/2/release-group?artist=<artist_mbid>&limit=100&offset=<offset>&type=album|ep|single&fmt=json`
- Browse releases by release group: `/ws/2/release?release-group=<release_group_mbid>&limit=100&fmt=json`

Notes:

- MusicBrainz is strong for structured metadata but not artist photos.
- Artist biographies are not a reliable first-class field.
- Discography should use release groups, not every release, to avoid flooding the UI with duplicate regional editions.

### Cover Art Archive

Use Cover Art Archive as the default album/release-group artwork source.

- API root: `https://coverartarchive.org/`
- No API key required
- Works directly with MusicBrainz release and release-group MBIDs
- Good thumbnail sizes: `250`, `500`, `1200`

Relevant endpoints:

- Release-group front image: `/release-group/<mbid>/front-250`
- Release-group metadata: `/release-group/<mbid>/`
- Release front image: `/release/<mbid>/front-250`
- Release metadata: `/release/<mbid>/`

Notes:

- Prefer release-group art for the artist discography grid.
- Fall back to release art when release-group art is missing.
- Cache downloaded images under `$XDG_CACHE_HOME/tempo/artwork` or `$XDG_CACHE_HOME/tempo/external-artwork`.

### TheAudioDB

Use TheAudioDB as the practical public source for artist bios and artist photos.

- V1 API base: `https://www.theaudiodb.com/api/v1/json/123/`
- Public free key is documented as `123`
- Free users are limited to 30 requests per minute
- Has MusicBrainz-ID lookups for artists and albums

Relevant endpoints:

- Artist by MusicBrainz ID: `/artist-mb.php?i=<artist_mbid>`
- Artist search: `/search.php?s=<artist_name>`
- Discography by MusicBrainz ID: `/discography-mb.php?s=<artist_mbid>`
- Album by MusicBrainz release-group ID: `/album-mb.php?i=<release_group_mbid>`

Useful fields:

- `strBiographyEN`
- `strArtistThumb`
- `strArtistFanart`
- `strArtistLogo`
- `strMusicBrainzID`

Notes:

- Use only after MusicBrainz has identified the artist when possible.
- Treat missing results as normal and cache misses with backoff.
- Artist photo priority should be `strArtistThumb`, then `strArtistFanart`.

### Discogs

Do not use Discogs as the default unauthenticated enrichment source.

- API root: `https://api.discogs.com`
- Requires meaningful `User-Agent`
- Unauthenticated rate limit is 25 requests per minute
- Authenticated rate limit is 60 requests per minute
- Image URLs require Discogs auth credentials or token

Notes:

- Discogs is useful for alternate album art, release variants, and richer release metadata.
- It should be optional behind user-provided credentials in settings.
- Do not block the main enrichment design on Discogs.

### AllMusic

Do not use AllMusic as a default source.

Notes:

- No stable public API was found for direct app integration.
- Scraping would be brittle and likely inappropriate.
- If needed later, only add it through an explicit plugin/source abstraction with clear user opt-in.

## Local Database Additions

The current `catalog.rs` schema already has base tables for artists, albums, assets, and metadata jobs. Online enrichment should extend or use these fields.

Implemented additions:

```sql
CREATE TABLE IF NOT EXISTS discography_items (
  id INTEGER PRIMARY KEY,
  artist_id INTEGER NOT NULL REFERENCES artists(id),
  title TEXT NOT NULL,
  normalized_title TEXT NOT NULL,
  year TEXT,
  release_type TEXT,
  musicbrainz_release_group_id TEXT UNIQUE,
  cover_asset_id INTEGER REFERENCES assets(id),
  local_album_id INTEGER REFERENCES albums(id),
  is_local INTEGER NOT NULL DEFAULT 0,
  sort_key TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS discography_artist_idx
ON discography_items(artist_id, sort_key);
```

The existing `metadata_jobs` table should be used for all network work.

`catalog.rs` now includes the `discography_items` table, a pending-job index, and helper APIs to enqueue, claim, complete, fail, upsert, and load enrichment records. Network fetching and settings gates are still intentionally separate so the app remains local-first until online metadata is explicitly enabled.

Recommended job types:

- `resolve_artist_musicbrainz`
- `fetch_artist_profile`
- `fetch_artist_discography`
- `resolve_album_musicbrainz`
- `fetch_album_cover`

Recommended statuses:

- `pending`
- `running`
- `complete`
- `failed`
- `blocked`

## Enrichment Flow

### Artist Seen During Ingest

1. Local scan upserts artist by normalized name.
2. If `artists.musicbrainz_id` is missing, enqueue `resolve_artist_musicbrainz`.
3. If artist profile is missing, enqueue `fetch_artist_profile`.
4. If discography is missing or stale, enqueue `fetch_artist_discography`.

### Album Seen During Ingest

1. Local scan upserts album by normalized title plus artist.
2. If album has no cover asset and no local/folder art, enqueue `resolve_album_musicbrainz`.
3. Once a release-group MBID exists, enqueue `fetch_album_cover`.

### Artist Discography Page

1. Show local artist data immediately.
2. If discography cache exists, render it immediately.
3. Local albums render at normal opacity with a local/downloaded indicator.
4. Missing albums render dimmed.
5. If discography is missing or stale, enqueue refresh but do not block the page.

## Matching Rules

Artist matching should prefer:

1. Existing MusicBrainz IDs from tags, if later added to `library::Track`.
2. Exact normalized artist name match from MusicBrainz search.
3. High-scoring MusicBrainz result with matching country/type/disambiguation where available.
4. Manual user correction later.

Album matching should prefer:

1. Existing release-group MBID from tags, if later added.
2. MusicBrainz release group by artist MBID plus normalized album title.
3. Release date/year proximity when the local tag has a year.
4. Manual user correction later.

Discography local ownership should match by:

1. `musicbrainz_release_group_id` when present.
2. Normalized title plus artist ID.
3. Normalized title plus artist name as fallback.

## Asset Cache Rules

- Store image files on disk, not as SQLite blobs.
- Store DB rows in `assets` with `kind`, `source`, `source_url`, `cache_path`, `content_hash`, `mime_type`, `status`, and `fetched_at`.
- Use source URLs or MBIDs for deterministic file names when possible.
- Use content hash for deduplication when downloading bytes.
- Never redownload when `cache_path` exists and `assets.status = 'ready'`.
- Failed downloads should update `metadata_jobs.attempts`, `next_attempt_at`, and `last_error`.

## Rate Limiting

Use a single background enrichment worker with per-source throttles.

Recommended minimum delays:

- MusicBrainz: 1 request per second
- TheAudioDB: 2 seconds between requests, or token bucket capped at 30/minute
- Cover Art Archive: conservative 250-500ms delay despite no explicit limit
- Discogs: only if configured; obey rate-limit headers

Do not run online enrichment from the UI thread or scanner thread.

## Privacy And Settings

Tempo should remain local-first.

Recommended settings:

- `Online Metadata: Off | Manual | Automatic`
- `Download artist photos`
- `Download album art when local art is missing`
- `Fetch artist biographies`
- Optional Discogs token/key configuration later

Default should be conservative. A good default is `Manual` or an onboarding prompt before first online lookup.

## Implementation Order

1. Add `discography_items` migration.
2. Add a `metadata_worker` module that claims pending jobs from SQLite.
3. Add MusicBrainz artist resolution.
4. Add TheAudioDB artist profile fetch for bio/photo.
5. Add MusicBrainz release-group discography fetch.
6. Add Cover Art Archive cover fetch.
7. Add artist detail page with cached bio and discography.
8. Add settings to enable/disable online enrichment.
9. Add manual refresh actions for artist and album pages.

## Failure Handling

- Cache misses are not errors in the UI.
- Persist failed attempts and use exponential backoff.
- Do not repeatedly query sources for unknown artists/albums during every scan.
- Surface only high-level status in the UI, such as `Metadata lookup pending` or `No online metadata found`.
