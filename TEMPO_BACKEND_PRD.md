# Tempo Backend PRD

## Goal

Build a simple Tempo metadata backend that proxies third-party metadata services, caches responses, and gives the desktop app a stable default metadata API.

The backend should reduce duplicate upstream requests, make metadata sync faster for users over time, centralize provider throttling, and keep provider-specific quirks out of the desktop client.

## Problem

Tempo currently talks directly to public metadata providers. That creates several issues:

- Each Tempo install repeats the same MusicBrainz, TheAudioDB, and Cover Art Archive requests.
- Metadata sync is slow because every user independently hits provider throttles.
- Provider errors, schema quirks, and matching logic live in the desktop app.
- Album image bytes are repeatedly downloaded from upstream providers.
- There is no shared cache across users or devices.

## Decision Summary

- Use a Tempo metadata proxy as the default metadata backend.
- Keep direct provider lookup as an opt-out setting.
- Do not impose product-level client rate limits for normal cached proxy reads.
- Enforce upstream provider throttles only when the proxy has a cache miss.
- Cache both JSON metadata and image bytes.
- Return normalized Tempo-facing responses from the proxy.
- Store raw upstream payloads internally for debugging and future re-normalization.
- Use FastAPI for the first implementation.
- Use Postgres as the durable metadata cache.
- Use Redis for provider throttles, hot cache, and in-flight request coalescing.

## Non-Goals

Initial version should not include:

- User accounts.
- Personalized recommendations.
- Uploading full local library manifests.
- Authenticated Discogs support.
- Metadata editing.
- Complex admin UI.
- Billing or paid API controls.
- A full rewrite of Tempo's local SQLite cache.

## Service Stack

Recommended MVP stack:

- FastAPI
- `httpx.AsyncClient` for upstream HTTP
- Pydantic for normalized response schemas
- Postgres for durable cache entries and metadata records
- Redis for rate limiting, hot cache, and request coalescing locks
- Object storage or filesystem storage for image bytes
- ProxyP2P for upstream provider requests from the backend

FastAPI is acceptable because this service is primarily I/O-bound. Provider normalization and cache behavior will likely change frequently early on, so iteration speed matters more than using the same language as the Rust desktop app.

## Upstream Proxy Layer

The Tempo backend should not call MusicBrainz, TheAudioDB, or Cover Art Archive directly in production. It should route cache-miss upstream requests through ProxyP2P.

ProxyP2P details:

```text
Production base URL: https://proxyp2p.com
Local default: http://localhost:8080
Request endpoint: POST /api/v1/request?organization_id=<org_id>
```

Backend environment variables:

```text
PROXYP2P_API_KEY="your-api-key"
PROXYP2P_API_URL="https://proxyp2p.com"
PROXYP2P_ORG_ID="1"
```

ProxyP2P request shape:

```json
{
  "url": "https://target.example/path",
  "method": "GET",
  "headers": {
    "Accept": "application/json"
  },
  "body": "optional request body"
}
```

ProxyP2P successful response shape:

```json
{
  "status_code": 200,
  "body": "{\"objectClassName\":\"domain\"}",
  "headers": {
    "Content-Type": "application/json"
  },
  "response_time_ms": 123.45
}
```

ProxyP2P error response shape:

```json
{
  "error": "error message",
  "response_time_ms": 12.34
}
```

The backend should wrap ProxyP2P behind an internal `UpstreamHttpClient` abstraction so provider clients do not know whether requests are direct or proxied.

Production behavior:

- Cache hit: return Tempo backend cache without ProxyP2P.
- Cache miss: wait for provider throttle, then send the upstream request through ProxyP2P.
- Store ProxyP2P `status_code`, `headers`, `body`, and `response_time_ms` with the cache entry or asset fetch record.
- Treat ProxyP2P transport errors separately from provider HTTP errors.

Development behavior:

- `PROXYP2P_API_URL` may point to `http://localhost:8080`.
- A direct HTTP fallback can be useful for local development, but production should use ProxyP2P for external provider requests.

## Providers

Initial supported upstream providers:

- MusicBrainz
- TheAudioDB
- Cover Art Archive

Future optional providers:

- Discogs, only with explicit credentials/support.

## Provider Responsibilities

### MusicBrainz

Primary source for structured identity and discography data.

Used for:

- Artist search and resolution.
- Artist lookup by MBID.
- Album/release-group resolution.
- Artist release-group discography.

Upstream throttle:

- 1 request per second globally.

### TheAudioDB

Practical source for artist profile metadata.

Used for:

- Artist biography by MusicBrainz ID.
- Artist photo by MusicBrainz ID.
- Optional artist search fallback.

Upstream throttle:

- 30 requests per minute, or conservative 1 request every 2 seconds.

### Cover Art Archive

Primary source for missing album art.

Used for:

- Release-group front image.
- Release front image fallback later.

Upstream throttle:

- No strict low public limit, but image bytes are bandwidth-heavy.
- Use conservative proxy-to-upstream throttling for cache misses.
- Cached image reads should not hit upstream.

## Tempo Client Behavior

Tempo should support two metadata modes:

- Tempo Metadata: default; uses the proxy.
- Direct Providers: opt-out; Tempo calls providers directly and enforces provider throttles locally.

Tempo should still:

- Respect `Online Metadata: Off | Automatic`.
- Cache fetched metadata locally in SQLite.
- Save downloaded album art locally into album folders when configured by the desktop app.
- Treat metadata failures as non-fatal.
- Continue playback and local library browsing without backend availability.

Suggested configuration:

```text
Metadata Backend:
- Tempo Metadata
- Direct Providers
```

Development override:

```text
TEMPO_METADATA_PROXY_URL=https://metadata.example.com
```

## API Shape

The backend should expose Tempo-focused routes rather than raw provider mirrors.

Initial routes:

```text
GET /v1/artists/search?name=<artist_name>
GET /v1/artists/:mbid
GET /v1/artists/:mbid/profile
GET /v1/artists/:mbid/discography

GET /v1/albums/resolve?artist_mbid=<mbid>&title=<album_title>
GET /v1/albums/:release_group_mbid
GET /v1/albums/:release_group_mbid/cover

GET /v1/health
GET /v1/cache/stats
```

Responses should include cache metadata:

```json
{
  "cache": "hit",
  "fetched_at": "2026-04-29T00:00:00Z",
  "expires_at": "2026-05-29T00:00:00Z",
  "sources": ["musicbrainz"],
  "data": {}
}
```

Valid cache states:

- `hit`
- `miss`
- `stale_revalidated`
- `stale_upstream_failed`
- `pending`

## Normalized Schemas

The proxy should return normalized response schemas that are stable for Tempo.

Raw provider payloads should be stored internally but not be the primary client contract.

### Artist Search Result

```json
{
  "artists": [
    {
      "musicbrainz_id": "uuid",
      "name": "Artist Name",
      "sort_name": "Artist Name",
      "disambiguation": null,
      "country": null,
      "score": 100,
      "source": "musicbrainz"
    }
  ]
}
```

### Artist Profile

```json
{
  "artist": {
    "musicbrainz_id": "uuid",
    "audiodb_id": "12345",
    "name": "Artist Name",
    "biography": "...",
    "biography_source": "theaudiodb",
    "photo_url": "https://metadata.example.com/v1/assets/...",
    "sources": ["musicbrainz", "theaudiodb"]
  }
}
```

### Artist Discography

```json
{
  "artist_musicbrainz_id": "uuid",
  "items": [
    {
      "title": "Album Title",
      "year": "1999",
      "release_type": "album",
      "musicbrainz_release_group_id": "uuid",
      "cover_url": null,
      "source": "musicbrainz"
    }
  ]
}
```

### Album Resolve

```json
{
  "album": {
    "title": "Album Title",
    "artist_musicbrainz_id": "uuid",
    "musicbrainz_release_group_id": "uuid",
    "year": "1999",
    "score": 100,
    "source": "musicbrainz"
  }
}
```

### Album Cover

```json
{
  "cover": {
    "musicbrainz_release_group_id": "uuid",
    "url": "https://metadata.example.com/v1/assets/...",
    "mime_type": "image/jpeg",
    "byte_size": 123456,
    "content_hash": "sha256:...",
    "source": "coverartarchive"
  }
}
```

## Caching

The cache should store both raw provider data and normalized Tempo-facing data.

Suggested Postgres table:

```sql
CREATE TABLE cache_entries (
  id BIGSERIAL PRIMARY KEY,
  cache_key TEXT NOT NULL UNIQUE,
  provider TEXT NOT NULL,
  endpoint TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  response_json JSONB,
  response_headers_json JSONB,
  normalized_json JSONB,
  fetched_at TIMESTAMPTZ,
  expires_at TIMESTAMPTZ,
  last_accessed_at TIMESTAMPTZ,
  access_count BIGINT NOT NULL DEFAULT 0,
  error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Suggested asset table:

```sql
CREATE TABLE assets (
  id BIGSERIAL PRIMARY KEY,
  cache_key TEXT NOT NULL UNIQUE,
  provider TEXT NOT NULL,
  source_url TEXT,
  mime_type TEXT,
  byte_size BIGINT,
  content_hash TEXT,
  storage_path TEXT NOT NULL,
  status TEXT NOT NULL,
  fetched_at TIMESTAMPTZ,
  expires_at TIMESTAMPTZ,
  last_accessed_at TIMESTAMPTZ,
  access_count BIGINT NOT NULL DEFAULT 0,
  error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

## Cache Keys

Use deterministic cache keys.

Examples:

```text
musicbrainz:artist-search:<normalized_artist_name>
musicbrainz:artist:<artist_mbid>
musicbrainz:release-groups:<artist_mbid>:<type>:<offset>
musicbrainz:release-group-search:<artist_mbid>:<normalized_album_title>
theaudiodb:artist-profile:<artist_mbid>
coverartarchive:release-group-front:<release_group_mbid>:500
```

## TTL Defaults

Suggested defaults:

- MusicBrainz identity data: 30 days.
- MusicBrainz discography: 14 days.
- TheAudioDB profiles: 30 days.
- Cover Art Archive images: effectively permanent, with manual invalidation support.
- Negative cache misses: 7 days.
- Upstream transient errors: exponential backoff.

## Request Flow

For a JSON metadata request:

1. Tempo requests a normalized endpoint.
2. Proxy computes cache key.
3. Proxy checks Redis hot cache, then Postgres durable cache.
4. If fresh cache exists, return immediately.
5. If stale cache exists, return stale data when acceptable and revalidate in background or inline based on endpoint needs.
6. If no cache exists, acquire an in-flight lock for the cache key.
7. If lock is acquired, wait for provider throttle, call upstream through ProxyP2P, normalize, store raw and normalized data, then return.
8. If lock is already held, wait briefly for cache fill or return `pending`.
9. If upstream fails and stale cache exists, return stale cache with `stale_upstream_failed`.
10. If upstream fails and no cache exists, return structured error.

For image requests:

1. Tempo requests album or artist image metadata.
2. Proxy checks asset cache.
3. If cached, return cached asset URL or bytes.
4. If missing, acquire lock and fetch upstream through ProxyP2P under provider throttle.
5. Store bytes in asset storage.
6. Return stable cached asset URL.

## Request Coalescing

The backend must prevent cache stampedes.

Use Redis locks keyed by cache key:

```text
lock:<cache_key>
```

Behavior:

- First miss holder fetches upstream.
- Concurrent requests wait briefly for the cache to fill.
- If the cache is still pending after a short timeout, return `pending` with `retry_after_seconds`.

## Rate Limiting

Tempo clients should not be rate-limited for normal cached reads.

The proxy should enforce upstream provider throttles only for cache misses:

- MusicBrainz: 1 request per second globally.
- TheAudioDB: 30 requests per minute, or 1 request every 2 seconds conservatively.
- Cover Art Archive: conservative image miss throttle.

Redis can store provider throttle state:

```text
throttle:musicbrainz
throttle:theaudiodb
throttle:coverartarchive
```

## Error Handling

Use structured errors:

```json
{
  "error": {
    "code": "upstream_unavailable",
    "message": "MusicBrainz request failed",
    "retry_after_seconds": 300
  }
}
```

Rules:

- Cache upstream 404/miss results to avoid repeated misses.
- Use exponential backoff for transient upstream errors.
- Serve stale cache when possible.
- Never fail Tempo playback or local browsing because metadata is unavailable.

## Privacy

Metadata proxy requests can reveal artist and album names.

Requirements:

- Do not require user identity for basic metadata lookup.
- Do not upload local file paths.
- Do not upload full local library manifests.
- Avoid long-term logs that pair IP addresses with raw queries.
- Document that artist/album lookup terms are sent to Tempo metadata services by default.
- Provide a direct-provider opt-out setting.

## Observability

Track basic metrics:

```text
cache_hit_count
cache_miss_count
cache_stale_served_count
upstream_request_count
upstream_error_count
proxyp2p_error_count
proxyp2p_average_latency_ms
provider_rate_limited_count
average_upstream_latency_ms
asset_cache_size_bytes
inflight_lock_wait_count
```

Health endpoints:

```text
GET /v1/health
GET /v1/cache/stats
```

## MVP Scope

MVP should include:

- FastAPI service.
- Postgres cache store.
- Redis provider throttles and in-flight locks.
- ProxyP2P upstream HTTP client integration.
- MusicBrainz artist search/resolve.
- TheAudioDB artist profile lookup by MBID.
- MusicBrainz album/release-group resolve.
- Cover Art Archive release-group cover cache.
- Raw payload storage.
- Normalized Tempo-facing responses.
- Basic health/stats endpoints.
- Tempo client support for proxy backend and direct-provider opt-out.

## Open Questions

1. Will the production proxy URL be bundled in Tempo or provided by config first?
2. Should image endpoints return bytes directly in MVP or return stable asset URLs?
3. Should stale cache be returned immediately for all endpoints, or only for selected endpoints?
4. What storage backend should image bytes use initially: local disk, S3-compatible object storage, or Postgres large objects?
5. What is the retention policy for raw upstream payloads and request logs?
