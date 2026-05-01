use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use reqwest::{
    blocking::{Client, RequestBuilder},
    header::{CONTENT_TYPE, RETRY_AFTER},
};
use serde::Deserialize;

use crate::{catalog::CatalogStore, perf};

const MUSICBRAINZ_ARTIST_RESOLVE: &str = "resolve_artist_musicbrainz";
const FETCH_ARTIST_PROFILE: &str = "fetch_artist_profile";
const FETCH_ARTIST_DISCOGRAPHY: &str = "fetch_artist_discography";
const RESOLVE_ALBUM_MUSICBRAINZ: &str = "resolve_album_musicbrainz";
const FETCH_ALBUM_COVER: &str = "fetch_album_cover";
const FETCH_ALBUM_PROFILE: &str = "fetch_album_profile";
const RESOLVE_ARTIST_AUDIODB_SEARCH: &str = "resolve_artist_audiodb_search";
const RESOLVE_ALBUM_AUDIODB_SEARCH: &str = "resolve_album_audiodb_search";
const FETCH_ARTIST_WIKIPEDIA_SUMMARY: &str = "fetch_artist_wikipedia_summary";
const FETCH_ALBUM_WIKIPEDIA_SUMMARY: &str = "fetch_album_wikipedia_summary";
const RESOLVE_ARTIST_DISCOGS_SEARCH: &str = "resolve_artist_discogs_search";
const FETCH_ARTIST_DISCOGS_PROFILE: &str = "fetch_artist_discogs_profile";
const FETCH_ARTIST_DISCOGS_RELEASES: &str = "fetch_artist_discogs_releases";
const RESOLVE_ALBUM_DISCOGS_SEARCH: &str = "resolve_album_discogs_search";
const FETCH_ALBUM_DISCOGS_IMAGE: &str = "fetch_album_discogs_image";
const FETCH_DISCOGS_THUMB: &str = "fetch_discogs_thumb";
/// Lidarr's metadata proxy at `api.lidarr.audio` is a strict superset
/// of MusicBrainz + TheAudioDB + Wikipedia + Discogs for the data we
/// care about (overview/bio, images by `CoverType`, full discography,
/// genres). It runs above all other sources in the chain because:
///   - one HTTP round-trip returns everything we need per artist or
///     album (vs. 4-5 across the legacy chain);
///   - responses are Cloudflare-cached for 30 days and the API
///     advertises 60 req/min per IP, so per-source contention is a
///     non-issue;
///   - it itself is backed by MusicBrainz, so an MBID is always the
///     lookup key.
const FETCH_ARTIST_LIDARR: &str = "fetch_artist_lidarr";
const FETCH_ALBUM_LIDARR: &str = "fetch_album_lidarr";
const SUPPORTED_JOB_TYPES: &[&str] = &[
    MUSICBRAINZ_ARTIST_RESOLVE,
    FETCH_ARTIST_LIDARR,
    FETCH_ALBUM_LIDARR,
    FETCH_ARTIST_PROFILE,
    FETCH_ARTIST_DISCOGRAPHY,
    RESOLVE_ALBUM_MUSICBRAINZ,
    FETCH_ALBUM_COVER,
    FETCH_ALBUM_PROFILE,
    RESOLVE_ARTIST_AUDIODB_SEARCH,
    RESOLVE_ALBUM_AUDIODB_SEARCH,
    FETCH_ARTIST_WIKIPEDIA_SUMMARY,
    FETCH_ALBUM_WIKIPEDIA_SUMMARY,
    RESOLVE_ARTIST_DISCOGS_SEARCH,
    FETCH_ARTIST_DISCOGS_PROFILE,
    FETCH_ARTIST_DISCOGS_RELEASES,
    RESOLVE_ALBUM_DISCOGS_SEARCH,
    FETCH_ALBUM_DISCOGS_IMAGE,
    FETCH_DISCOGS_THUMB,
];
const WORKER_IDLE_DELAY: Duration = Duration::from_secs(10);
const MUSICBRAINZ_DELAY: Duration = Duration::from_secs(1);
const THEAUDIODB_DELAY: Duration = Duration::from_secs(2);
const WIKIPEDIA_DELAY: Duration = Duration::from_millis(250);
/// Discogs unauthenticated rate limit is 25 req/min. 60s / 25 = 2.4s
/// per request; the worker enforces this via `wait_for_rate_limit`.
const DISCOGS_DELAY: Duration = Duration::from_millis(2_400);
/// Lidarr's hosted proxy advertises 60 req/min. We use 1.0s spacing
/// to stay well under that and to keep the worker responsive when
/// Lidarr is doing heavy lifting on the server side (uncached lookups
/// can take 1-3s to roundtrip).
const LIDARR_DELAY: Duration = Duration::from_secs(1);
const LIDARR_API_BASE: &str = "https://api.lidarr.audio/api/v0.4";
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
const MUSICBRAINZ_RELEASE_GROUP_LIMIT: usize = 100;
const MUSICBRAINZ_RELEASE_GROUP_MAX_PAGES: usize = 5;
/// Page size for `/artists/{id}/releases`. Discogs caps at 100 per
/// page; we paginate up to `DISCOGS_RELEASES_MAX_PAGES` to stay under
/// a few minutes of work for any single artist.
const DISCOGS_RELEASES_LIMIT: usize = 100;
const DISCOGS_RELEASES_MAX_PAGES: usize = 5;
const HTTP_BODY_EXCERPT_MAX: usize = 512;
const PARSE_BODY_EXCERPT_MAX: usize = 256;
const USER_AGENT: &str = concat!(
    "Tempo/",
    env!("CARGO_PKG_VERSION"),
    " (local music library app)"
);

#[derive(Debug)]
pub struct MetadataWorker {
    shutdown_tx: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug)]
pub enum MetadataEvent {
    ArtistUpdated(i64),
    AlbumUpdated(i64),
}

impl MetadataWorker {
    pub fn start(catalog: CatalogStore, events: mpsc::Sender<MetadataEvent>) -> Result<Self> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("tempo-metadata-worker".to_string())
            .spawn(move || run_worker(catalog, events, shutdown_rx))
            .context("failed to spawn metadata worker thread")?;

        Ok(Self {
            shutdown_tx,
            handle: Some(handle),
        })
    }

    pub fn stop(mut self) {
        let _span = perf::span("metadata.worker.stop", "");
        let _ = self.shutdown_tx.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MetadataWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

#[derive(Clone, Copy, Debug)]
enum MetadataErrorKind {
    Network,
    Http4xx,
    Http5xx,
    Parse,
    #[allow(dead_code)]
    NoMatch,
}

impl MetadataErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Http4xx => "http_4xx",
            Self::Http5xx => "http_5xx",
            Self::Parse => "parse",
            Self::NoMatch => "no_match",
        }
    }
}

#[derive(Debug)]
struct MetadataApiError {
    kind: MetadataErrorKind,
    source: &'static str,
    message: String,
    status: Option<u16>,
    #[allow(dead_code)]
    retry_after: Option<Duration>,
}

impl std::fmt::Display for MetadataApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MetadataApiError {}

fn redact_authorization(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("authorization:") {
            out.push_str("Authorization: <redacted>");
            if line.ends_with("\r\n") {
                out.push_str("\r\n");
            } else if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

fn body_excerpt(text: &str, max: usize) -> String {
    let redacted = redact_authorization(text);
    if redacted.len() <= max {
        redacted
    } else {
        let mut excerpt: String = redacted.chars().take(max).collect();
        excerpt.push_str("...[truncated]");
        excerpt
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Sends `request`, returns the response on success, or a populated
/// `MetadataApiError` on transport / non-2xx. The fallback `url` is used
/// only when the request couldn't be cloned for context capture.
fn send_capturing(
    request: RequestBuilder,
    source: &'static str,
) -> std::result::Result<reqwest::blocking::Response, MetadataApiError> {
    let cloned = request.try_clone();
    let fallback_url = cloned
        .as_ref()
        .and_then(|r| r.try_clone())
        .and_then(|r| r.build().ok())
        .map(|req| req.url().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());

    let response = match request.send() {
        Ok(response) => response,
        Err(error) => {
            let message = format!("{source} network error: {error}");
            perf::event(
                "metadata.api.http_error",
                format!("source={source} kind=network url={fallback_url} error={error}"),
            );
            return Err(MetadataApiError {
                kind: MetadataErrorKind::Network,
                source,
                message,
                status: None,
                retry_after: None,
            });
        }
    };

    let status = response.status();
    if !status.is_success() {
        let status_code = status.as_u16();
        let url = response.url().to_string();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);

        let body_text = response.text().unwrap_or_default();
        let excerpt = body_excerpt(&body_text, HTTP_BODY_EXCERPT_MAX);

        let kind = if (400..500).contains(&status_code) {
            MetadataErrorKind::Http4xx
        } else if status_code >= 500 {
            MetadataErrorKind::Http5xx
        } else {
            MetadataErrorKind::Network
        };

        perf::event(
            "metadata.api.http_error",
            format!("source={source} status={status_code} url={url} body={excerpt}"),
        );

        let message = format!("{source} {status_code}: {url}");
        return Err(MetadataApiError {
            kind,
            source,
            message,
            status: Some(status_code),
            retry_after,
        });
    }

    Ok(response)
}

fn request_json<T: serde::de::DeserializeOwned>(
    _client: &Client,
    request: RequestBuilder,
    source: &'static str,
) -> std::result::Result<T, MetadataApiError> {
    let response = send_capturing(request, source)?;
    let url = response.url().to_string();
    let bytes = match response.bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!("{source} body read error: {error}");
            perf::event(
                "metadata.api.http_error",
                format!("source={source} kind=network url={url} error={error}"),
            );
            return Err(MetadataApiError {
                kind: MetadataErrorKind::Network,
                source,
                message,
                status: None,
                retry_after: None,
            });
        }
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => Ok(value),
        Err(error) => {
            let body_text = String::from_utf8_lossy(&bytes);
            let excerpt = body_excerpt(&body_text, PARSE_BODY_EXCERPT_MAX);
            perf::event(
                "metadata.api.parse_error",
                format!("source={source} url={url} body={excerpt}"),
            );
            let message = format!("{source} parse error: {error}");
            Err(MetadataApiError {
                kind: MetadataErrorKind::Parse,
                source,
                message,
                status: None,
                retry_after: None,
            })
        }
    }
}

fn request_bytes(
    _client: &Client,
    request: RequestBuilder,
    source: &'static str,
) -> std::result::Result<(Option<String>, Vec<u8>), MetadataApiError> {
    let response = send_capturing(request, source)?;
    let mime_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let url = response.url().to_string();
    let data = match response.bytes() {
        Ok(bytes) => bytes.to_vec(),
        Err(error) => {
            let message = format!("{source} body read error: {error}");
            perf::event(
                "metadata.api.http_error",
                format!("source={source} kind=network url={url} error={error}"),
            );
            return Err(MetadataApiError {
                kind: MetadataErrorKind::Network,
                source,
                message,
                status: None,
                retry_after: None,
            });
        }
    };
    Ok((mime_type, data))
}

fn classify_for_persistence(err: &anyhow::Error) -> (Option<&'static str>, Option<&'static str>) {
    for cause in err.chain() {
        if let Some(api_err) = cause.downcast_ref::<MetadataApiError>() {
            return (Some(api_err.kind.as_str()), Some(api_err.source));
        }
    }
    (None, None)
}

fn run_worker(
    catalog: CatalogStore,
    events: mpsc::Sender<MetadataEvent>,
    shutdown_rx: mpsc::Receiver<()>,
) {
    let _span = perf::span("metadata.worker.run", "");
    let Ok(client) = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
    else {
        perf::event("metadata.worker.client_error", "failed_to_build_client");
        return;
    };
    let mut last_musicbrainz_request: Option<Instant> = None;
    let mut last_audiodb_request: Option<Instant> = None;
    let mut last_wikipedia_request: Option<Instant> = None;
    let mut last_discogs_request: Option<Instant> = None;
    let mut last_lidarr_request: Option<Instant> = None;

    match catalog.reset_stale_metadata_jobs() {
        Ok(count) if count > 0 => {
            perf::event("metadata.worker.reset_stale", format!("jobs={count}"));
        }
        Ok(_) => {}
        Err(error) => perf::event("metadata.worker.reset_stale_error", format!("{error:#}")),
    }

    // Aggressive resync: walks every artist/album that's still short
    // of a bio/photo/description/cover and enqueues the next link in
    // the fallback chain (Wikipedia, Discogs, etc.). Replaces the
    // earlier `enqueue_missing_online_metadata_jobs` backfill which
    // only ever re-armed `fetch_artist_profile` -- a no-op once that
    // job had completed regardless of whether it produced a bio.
    match catalog.resync_metadata_enrichment() {
        Ok(count) => perf::event("metadata.worker.resync", format!("jobs={count}")),
        Err(error) => perf::event("metadata.worker.resync_error", format!("{error:#}")),
    }

    loop {
        if shutdown_rx.try_recv().is_ok() {
            return;
        }

        let job = match catalog.claim_next_metadata_job(SUPPORTED_JOB_TYPES) {
            Ok(job) => job,
            Err(error) => {
                perf::event("metadata.worker.claim_error", format!("{error:#}"));
                if shutdown_rx.recv_timeout(WORKER_IDLE_DELAY).is_ok() {
                    return;
                }
                continue;
            }
        };

        let Some(job) = job else {
            if shutdown_rx.recv_timeout(WORKER_IDLE_DELAY).is_ok() {
                return;
            }
            continue;
        };

        let result = match job.job_type.as_str() {
            MUSICBRAINZ_ARTIST_RESOLVE => {
                if !wait_for_rate_limit(
                    &mut last_musicbrainz_request,
                    MUSICBRAINZ_DELAY,
                    &shutdown_rx,
                ) {
                    return;
                }
                resolve_artist_musicbrainz(&catalog, &client, job.entity_id)
            }
            FETCH_ARTIST_PROFILE => {
                if !wait_for_rate_limit(&mut last_audiodb_request, THEAUDIODB_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_artist_profile(&catalog, &client, job.entity_id)
            }
            FETCH_ARTIST_DISCOGRAPHY => {
                if !wait_for_rate_limit(
                    &mut last_musicbrainz_request,
                    MUSICBRAINZ_DELAY,
                    &shutdown_rx,
                ) {
                    return;
                }
                fetch_artist_discography(&catalog, &client, job.entity_id, &shutdown_rx)
            }
            RESOLVE_ALBUM_MUSICBRAINZ => {
                if !wait_for_rate_limit(
                    &mut last_musicbrainz_request,
                    MUSICBRAINZ_DELAY,
                    &shutdown_rx,
                ) {
                    return;
                }
                resolve_album_musicbrainz(&catalog, &client, job.entity_id)
            }
            FETCH_ALBUM_COVER => fetch_album_cover(&catalog, &client, job.entity_id),
            FETCH_ALBUM_PROFILE => {
                if !wait_for_rate_limit(&mut last_audiodb_request, THEAUDIODB_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_album_profile(&catalog, &client, job.entity_id)
            }
            RESOLVE_ARTIST_AUDIODB_SEARCH => {
                if !wait_for_rate_limit(&mut last_audiodb_request, THEAUDIODB_DELAY, &shutdown_rx) {
                    return;
                }
                resolve_artist_audiodb_search(&catalog, &client, job.entity_id)
            }
            RESOLVE_ALBUM_AUDIODB_SEARCH => {
                if !wait_for_rate_limit(&mut last_audiodb_request, THEAUDIODB_DELAY, &shutdown_rx) {
                    return;
                }
                resolve_album_audiodb_search(&catalog, &client, job.entity_id)
            }
            // The two Wikipedia-summary jobs perform two paced calls
            // (a MusicBrainz `inc=url-rels` lookup, then the Wikipedia
            // REST summary). Each function gates its own rate limits
            // internally, so the dispatch arm doesn't pre-wait.
            FETCH_ARTIST_WIKIPEDIA_SUMMARY => fetch_artist_wikipedia_summary(
                &catalog,
                &client,
                job.entity_id,
                &mut last_musicbrainz_request,
                &mut last_wikipedia_request,
                &shutdown_rx,
            ),
            FETCH_ALBUM_WIKIPEDIA_SUMMARY => fetch_album_wikipedia_summary(
                &catalog,
                &client,
                job.entity_id,
                &mut last_musicbrainz_request,
                &mut last_wikipedia_request,
                &shutdown_rx,
            ),
            // Discogs single-request jobs gate at dispatch level. The
            // multi-request `fetch_artist_discogs_releases` paces
            // internally, so the dispatch arm hands it the slot
            // without pre-waiting.
            RESOLVE_ARTIST_DISCOGS_SEARCH => {
                if !wait_for_rate_limit(&mut last_discogs_request, DISCOGS_DELAY, &shutdown_rx) {
                    return;
                }
                resolve_artist_discogs_search(&catalog, &client, job.entity_id)
            }
            FETCH_ARTIST_DISCOGS_PROFILE => {
                if !wait_for_rate_limit(&mut last_discogs_request, DISCOGS_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_artist_discogs_profile(&catalog, &client, job.entity_id)
            }
            FETCH_ARTIST_DISCOGS_RELEASES => fetch_artist_discogs_releases(
                &catalog,
                &client,
                job.entity_id,
                &mut last_discogs_request,
                &shutdown_rx,
            ),
            RESOLVE_ALBUM_DISCOGS_SEARCH => {
                if !wait_for_rate_limit(&mut last_discogs_request, DISCOGS_DELAY, &shutdown_rx) {
                    return;
                }
                resolve_album_discogs_search(&catalog, &client, job.entity_id)
            }
            FETCH_ALBUM_DISCOGS_IMAGE => {
                if !wait_for_rate_limit(&mut last_discogs_request, DISCOGS_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_album_discogs_image(&catalog, &client, job.entity_id)
            }
            FETCH_DISCOGS_THUMB => {
                if !wait_for_rate_limit(&mut last_discogs_request, DISCOGS_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_discogs_thumb(&catalog, &client, job.entity_id)
            }
            FETCH_ARTIST_LIDARR => {
                if !wait_for_rate_limit(&mut last_lidarr_request, LIDARR_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_artist_lidarr(&catalog, &client, job.entity_id)
            }
            FETCH_ALBUM_LIDARR => {
                if !wait_for_rate_limit(&mut last_lidarr_request, LIDARR_DELAY, &shutdown_rx) {
                    return;
                }
                fetch_album_lidarr(&catalog, &client, job.entity_id)
            }
            unsupported => Err(anyhow!("unsupported metadata job type: {unsupported}")),
        };

        match result {
            Ok(()) => {
                if let Err(error) = catalog.complete_metadata_job(job.job_id) {
                    perf::event("metadata.worker.complete_error", format!("{error:#}"));
                }
                match job.entity_type.as_str() {
                    "artist" => {
                        let _ = events.send(MetadataEvent::ArtistUpdated(job.entity_id));
                    }
                    "album" => {
                        let _ = events.send(MetadataEvent::AlbumUpdated(job.entity_id));
                    }
                    _ => {}
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                match job.entity_type.as_str() {
                    "artist" => {
                        let _ = catalog.mark_artist_metadata_checked(
                            job.entity_id,
                            "error",
                            Some(&message),
                        );
                    }
                    "album" => {
                        let _ = catalog.mark_album_metadata_checked(
                            job.entity_id,
                            "error",
                            Some(&message),
                        );
                    }
                    _ => {}
                }
                let (kind_str, source_str) = classify_for_persistence(&error);
                if let Err(error) =
                    catalog.fail_metadata_job_classified(job.job_id, &message, kind_str, source_str)
                {
                    perf::event("metadata.worker.fail_error", format!("{error:#}"));
                }
            }
        }
    }
}

fn wait_for_rate_limit(
    last_request: &mut Option<Instant>,
    delay: Duration,
    shutdown_rx: &mpsc::Receiver<()>,
) -> bool {
    if let Some(last_request) = *last_request {
        let elapsed = last_request.elapsed();
        if elapsed < delay && shutdown_rx.recv_timeout(delay - elapsed).is_ok() {
            return false;
        }
    }
    *last_request = Some(Instant::now());
    true
}

fn resolve_artist_musicbrainz(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    if artist.musicbrainz_id.is_some() {
        enqueue_artist_enrichment_jobs(catalog, artist.artist_id)?;
        return Ok(());
    }

    let request = client.get("https://musicbrainz.org/ws/2/artist").query(&[
        ("query", artist.name.as_str()),
        ("fmt", "json"),
        ("limit", "10"),
    ]);
    let response: MusicBrainzArtistSearch =
        request_json(client, request, "musicbrainz").map_err(anyhow::Error::from)?;

    let Some(match_) = best_artist_match(&artist.normalized_name, &response.artists) else {
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_AUDIODB_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("No MusicBrainz artist match"),
        )?;
        return Ok(());
    };

    catalog.resolve_artist_musicbrainz_id(artist.artist_id, &match_.id)?;
    enqueue_artist_enrichment_jobs(catalog, artist.artist_id)?;
    let _ = catalog.enqueue_missing_album_art_jobs_for_artist(artist.artist_id);
    Ok(())
}

fn enqueue_artist_enrichment_jobs(catalog: &CatalogStore, artist_id: i64) -> Result<()> {
    // Lidarr-first: one round-trip covers bio + photo + discography
    // for the common case. The legacy MB+TADb chain still runs as a
    // fallback because Lidarr's overview can be empty for less-popular
    // artists.
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_LIDARR)?;
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_PROFILE)?;
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_DISCOGRAPHY)?;
    Ok(())
}

/// Enqueue the Discogs-side enrichment chain for an artist that has a
/// resolved `discogs_id`. Kept separate from
/// [`enqueue_artist_enrichment_jobs`] so the MB-only path doesn't
/// accidentally fan out Discogs jobs for artists that have no Discogs
/// id yet (those would just no-op as `blocked`).
fn enqueue_artist_discogs_jobs(catalog: &CatalogStore, artist_id: i64) -> Result<()> {
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_DISCOGS_PROFILE)?;
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_DISCOGS_RELEASES)?;
    Ok(())
}

fn fetch_artist_profile(catalog: &CatalogStore, client: &Client, artist_id: i64) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(musicbrainz_id) = artist.musicbrainz_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no MusicBrainz ID"),
        )?;
        return Ok(());
    };

    let request = client
        .get("https://www.theaudiodb.com/api/v1/json/123/artist-mb.php")
        .query(&[("i", musicbrainz_id)]);
    let response: AudioDbArtistResponse =
        request_json(client, request, "theaudiodb").map_err(anyhow::Error::from)?;

    let Some(profile) = response.artists.into_iter().flatten().next() else {
        catalog.enqueue_metadata_job("artist", artist.artist_id, FETCH_ARTIST_WIKIPEDIA_SUMMARY)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("No TheAudioDB artist profile"),
        )?;
        return Ok(());
    };

    let photo_url = profile
        .artist_thumb
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| {
            profile
                .artist_fanart
                .as_deref()
                .filter(|url| !url.trim().is_empty())
        });
    let photo_asset_id = photo_url
        .map(|url| download_external_asset(catalog, client, "artist_photo", "theaudiodb", url))
        .transpose()?;

    let bio = profile
        .biography_en
        .as_deref()
        .map(str::trim)
        .filter(|bio| !bio.is_empty());

    catalog.save_artist_profile(
        artist.artist_id,
        profile.artist_id.as_deref(),
        bio,
        photo_asset_id,
        Some("theaudiodb"),
    )?;

    if bio.is_none() {
        // TheAudioDB returned a profile shell (likely with photos) but
        // no English bio; queue Wikipedia as a follow-up so the bio
        // surface eventually populates without re-running TADb.
        catalog.enqueue_metadata_job("artist", artist.artist_id, FETCH_ARTIST_WIKIPEDIA_SUMMARY)?;
    }
    Ok(())
}

fn fetch_artist_discography(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
    shutdown_rx: &mpsc::Receiver<()>,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(musicbrainz_id) = artist.musicbrainz_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no MusicBrainz ID"),
        )?;
        return Ok(());
    };

    let mut offset = 0usize;
    let mut saved_items = 0usize;
    let mut last_musicbrainz_request = Some(Instant::now());
    for page in 0..MUSICBRAINZ_RELEASE_GROUP_MAX_PAGES {
        if shutdown_rx.try_recv().is_ok() {
            return Ok(());
        }
        if page > 0
            && !wait_for_rate_limit(
                &mut last_musicbrainz_request,
                MUSICBRAINZ_DELAY,
                shutdown_rx,
            )
        {
            return Ok(());
        }

        let query = [
            ("artist", musicbrainz_id.to_string()),
            ("limit", MUSICBRAINZ_RELEASE_GROUP_LIMIT.to_string()),
            ("offset", offset.to_string()),
            ("type", "album|ep|single".to_string()),
            ("fmt", "json".to_string()),
        ];
        let request = client
            .get("https://musicbrainz.org/ws/2/release-group")
            .query(&query);
        let response: MusicBrainzReleaseGroupResponse =
            request_json(client, request, "musicbrainz").map_err(anyhow::Error::from)?;

        let page_len = response.release_groups.len();
        for release_group in response.release_groups {
            let release_type = release_group
                .primary_type
                .as_deref()
                .unwrap_or("album")
                .to_ascii_lowercase();
            catalog.upsert_discography_item(
                artist.artist_id,
                &release_group.title,
                release_group.year().as_deref(),
                &release_type,
                Some(&release_group.id),
            )?;
            saved_items += 1;
        }

        if page_len < MUSICBRAINZ_RELEASE_GROUP_LIMIT {
            break;
        }
        offset += MUSICBRAINZ_RELEASE_GROUP_LIMIT;
    }

    catalog.mark_artist_metadata_checked(
        artist.artist_id,
        if saved_items == 0 {
            "missing"
        } else {
            "resolved"
        },
        if saved_items == 0 {
            Some("No MusicBrainz release groups")
        } else {
            None
        },
    )?;
    Ok(())
}

fn resolve_album_musicbrainz(catalog: &CatalogStore, client: &Client, album_id: i64) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    if album.musicbrainz_release_group_id.is_some() {
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_LIDARR)?;
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_PROFILE)?;
        return Ok(());
    }
    let Some(artist_musicbrainz_id) = album.artist_musicbrainz_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album artist has no MusicBrainz ID"),
        )?;
        return Ok(());
    };

    let query = format!(
        "artistid:{} AND releasegroup:\"{}\"",
        artist_musicbrainz_id,
        album.title.replace('"', "")
    );
    let request = client
        .get("https://musicbrainz.org/ws/2/release-group")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "10")]);
    let response: MusicBrainzReleaseGroupResponse =
        request_json(client, request, "musicbrainz").map_err(anyhow::Error::from)?;

    let Some(match_) = best_release_group_match(&album.normalized_title, &response.release_groups)
    else {
        catalog.enqueue_metadata_job("album", album.album_id, RESOLVE_ALBUM_AUDIODB_SEARCH)?;
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No MusicBrainz release group match"),
        )?;
        return Ok(());
    };

    catalog.resolve_album_musicbrainz_release_group_id(album.album_id, &match_.id)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_LIDARR)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_PROFILE)?;
    Ok(())
}

fn fetch_album_cover(catalog: &CatalogStore, client: &Client, album_id: i64) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    let Some(release_group_id) = album.musicbrainz_release_group_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album has no MusicBrainz release group ID"),
        )?;
        return Ok(());
    };

    let url = format!("https://coverartarchive.org/release-group/{release_group_id}/front-500");
    let request = client.get(&url);
    match request_bytes(client, request, "coverartarchive") {
        Ok((mime_type, data)) => {
            catalog.save_album_cover_file(album.album_id, &url, mime_type.as_deref(), &data)?;
            Ok(())
        }
        Err(err) if err.status == Some(404) => {
            // CAA has no cover for this release group. Try Discogs as
            // the next-tier fallback. We unconditionally enqueue the
            // job; if the album has no `discogs_master_id` yet, the
            // job itself emits `blocked` and a Discogs master search
            // is expected to resolve it via the parallel
            // `resolve_album_discogs_search` chain queued from
            // `resolve_album_audiodb_search`.
            catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_DISCOGS_IMAGE)?;
            catalog.mark_album_metadata_checked(
                album.album_id,
                "missing",
                Some("No Cover Art Archive release group cover"),
            )?;
            Ok(())
        }
        Err(err) => Err(anyhow::Error::from(err)),
    }
}

/// TheAudioDB album profile fetch. Pulls `strDescriptionEN` (English
/// blurb) for the album by release-group MBID; persists it via
/// [`CatalogStore::save_album_profile`]. When the album has no
/// release-group MBID yet, the worker emits a `blocked` status so the
/// activity panel surfaces the unresolved state, mirroring the
/// `fetch_album_cover` semantics.
fn fetch_album_profile(catalog: &CatalogStore, client: &Client, album_id: i64) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    let Some(release_group_id) = album.musicbrainz_release_group_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album has no MusicBrainz release group ID"),
        )?;
        return Ok(());
    };

    let request = client
        .get("https://www.theaudiodb.com/api/v1/json/123/album-mb.php")
        .query(&[("i", release_group_id)]);
    let response: AudioDbAlbumResponse =
        request_json(client, request, "theaudiodb").map_err(anyhow::Error::from)?;

    let Some(profile) = response.album.into_iter().flatten().next() else {
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_WIKIPEDIA_SUMMARY)?;
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No TheAudioDB album profile"),
        )?;
        return Ok(());
    };

    let description = profile
        .description_en
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    if description.is_none() && profile.album_id.as_deref().is_none() {
        // Empty payload; record `missing` and queue Wikipedia as the
        // next-tier fallback so the description surface eventually
        // populates without re-querying TheAudioDB.
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_WIKIPEDIA_SUMMARY)?;
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("TheAudioDB album profile contained no description"),
        )?;
        return Ok(());
    }

    catalog.save_album_profile(
        album.album_id,
        profile.album_id.as_deref(),
        description,
        Some("theaudiodb"),
    )?;

    if description.is_none() {
        // Profile carried IDs/cover but no English blurb. Queue
        // Wikipedia so the description column gets backfilled.
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_WIKIPEDIA_SUMMARY)?;
    }
    Ok(())
}

/// TheAudioDB artist search by name. Used as the MB-resolver fallback
/// when `/ws/2/artist?query=` returns no acceptable match: TADb often
/// indexes underground artists with their MBID populated, so a hit
/// here lets us adopt the MBID and rerun the standard
/// `fetch_artist_profile` + `fetch_artist_discography` chain.
fn resolve_artist_audiodb_search(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    if artist.musicbrainz_id.is_some() {
        // Resolved by another path between enqueue and dispatch; skip
        // the search and just (re-)queue the standard enrichment chain.
        enqueue_artist_enrichment_jobs(catalog, artist.artist_id)?;
        return Ok(());
    }

    let request = client
        .get("https://www.theaudiodb.com/api/v1/json/123/search.php")
        .query(&[("s", artist.name.as_str())]);
    let response: AudioDbArtistResponse =
        request_json(client, request, "theaudiodb").map_err(anyhow::Error::from)?;

    let Some(profile) = response.artists.into_iter().flatten().next() else {
        // TADb didn't recognize the artist either. Fall through to
        // Discogs as the third-tier resolver before declaring the
        // artist `missing`.
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_DISCOGS_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("No TheAudioDB artist match"),
        )?;
        return Ok(());
    };

    let Some(mbid) = profile
        .musicbrainz_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        // TADb returned a hit without an MBID; chain to Discogs so the
        // bio + photo path still has another shot.
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_DISCOGS_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("TheAudioDB artist had no MusicBrainz ID"),
        )?;
        return Ok(());
    };

    catalog.resolve_artist_musicbrainz_id(artist.artist_id, mbid)?;
    enqueue_artist_enrichment_jobs(catalog, artist.artist_id)?;
    let _ = catalog.enqueue_missing_album_art_jobs_for_artist(artist.artist_id);
    Ok(())
}

/// TheAudioDB album search by `<artist>` + `<album>`. Mirror of
/// [`resolve_artist_audiodb_search`]: when MB's release-group lookup
/// fails, this attempts to surface the release-group MBID via TADb
/// and re-runs the `fetch_album_cover` + `fetch_album_profile` chain.
/// `searchalbum.php` does not always return a release-group MBID; an
/// empty `strMusicBrainzID` is treated as a true miss.
fn resolve_album_audiodb_search(
    catalog: &CatalogStore,
    client: &Client,
    album_id: i64,
) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    if album.musicbrainz_release_group_id.is_some() {
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_LIDARR)?;
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_PROFILE)?;
        return Ok(());
    }

    let request = client
        .get("https://www.theaudiodb.com/api/v1/json/123/searchalbum.php")
        .query(&[
            ("s", album.artist_name.as_str()),
            ("a", album.title.as_str()),
        ]);
    let response: AudioDbAlbumSearchResponse =
        request_json(client, request, "theaudiodb").map_err(anyhow::Error::from)?;

    let Some(profile) = response.album.into_iter().flatten().next() else {
        // No TADb hit either: hand off to Discogs master search as
        // the next-tier fallback.
        catalog.enqueue_metadata_job("album", album.album_id, RESOLVE_ALBUM_DISCOGS_SEARCH)?;
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No TheAudioDB album match"),
        )?;
        return Ok(());
    };

    let Some(mbid) = profile
        .musicbrainz_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        // TADb hit but no release-group MBID; try Discogs.
        catalog.enqueue_metadata_job("album", album.album_id, RESOLVE_ALBUM_DISCOGS_SEARCH)?;
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("TheAudioDB album had no release-group MBID"),
        )?;
        return Ok(());
    };

    catalog.resolve_album_musicbrainz_release_group_id(album.album_id, mbid)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_LIDARR)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_PROFILE)?;
    Ok(())
}

/// Given a `https://en.wikipedia.org/wiki/Some_Page_Title` URL, return
/// the `Some_Page_Title` segment in its existing percent-encoded form
/// (the REST summary endpoint accepts the same encoding the wiki URL
/// uses). Returns `None` for URLs with no `/wiki/` path or for empty
/// titles. The returned slice is borrowed from the input.
fn wikipedia_title_from_url(url: &str) -> Option<&str> {
    let after_scheme = url.split("//").nth(1)?;
    let after_host = after_scheme.split_once('/')?.1;
    let title = after_host.strip_prefix("wiki/")?;
    let title = title.split('#').next()?;
    let title = title.split('?').next()?;
    if title.is_empty() { None } else { Some(title) }
}

/// Pick the best Wikipedia URL from a `url-rels` block: prefers an
/// English-language page (`en.wikipedia.org`) and falls back to any
/// Wikipedia link if no English variant exists. Returns the
/// `resource` URL slice borrowed from `rels`.
fn pick_wikipedia_url(rels: &[MusicBrainzUrlRelation]) -> Option<&str> {
    let is_wikipedia =
        |r: &&MusicBrainzUrlRelation| r.relation_type.as_deref() == Some("wikipedia");
    rels.iter()
        .filter(is_wikipedia)
        .find(|r| r.url.resource.contains("en.wikipedia.org"))
        .or_else(|| rels.iter().find(is_wikipedia))
        .map(|r| r.url.resource.as_str())
}

/// Final-tier artist bio fallback. Looks up the artist's MusicBrainz
/// record with `inc=url-rels`, picks the best Wikipedia URL, and
/// stores the page's `extract` field as the bio with
/// `bio_source = 'wikipedia'`. Both the MB lookup and the Wikipedia
/// REST call are paced via the shared rate-limit slots so this job
/// can interleave with the discography crawler without exceeding
/// either provider's budget.
fn fetch_artist_wikipedia_summary(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
    last_musicbrainz_request: &mut Option<Instant>,
    last_wikipedia_request: &mut Option<Instant>,
    shutdown_rx: &mpsc::Receiver<()>,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(mbid) = artist.musicbrainz_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no MusicBrainz ID"),
        )?;
        return Ok(());
    };

    if !wait_for_rate_limit(last_musicbrainz_request, MUSICBRAINZ_DELAY, shutdown_rx) {
        return Ok(());
    }
    let request = client
        .get(format!("https://musicbrainz.org/ws/2/artist/{mbid}"))
        .query(&[("inc", "url-rels"), ("fmt", "json")]);
    let lookup: MusicBrainzArtistLookup =
        request_json(client, request, "musicbrainz").map_err(anyhow::Error::from)?;

    let Some(url) = pick_wikipedia_url(&lookup.relations) else {
        // Wikipedia exhausted: chain to Discogs as the final fallback
        // for artist bio + photo. The Discogs resolver will land a
        // discogs_id and chain to `fetch_artist_discogs_profile` /
        // `fetch_artist_discogs_releases` automatically.
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_DISCOGS_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("Artist has no Wikipedia URL in MusicBrainz"),
        )?;
        return Ok(());
    };
    let Some(title) = wikipedia_title_from_url(url) else {
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_DISCOGS_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("Wikipedia URL is unparseable"),
        )?;
        return Ok(());
    };

    if !wait_for_rate_limit(last_wikipedia_request, WIKIPEDIA_DELAY, shutdown_rx) {
        return Ok(());
    }
    let request = client.get(format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{title}"
    ));
    let summary: WikipediaSummary =
        request_json(client, request, "wikipedia").map_err(anyhow::Error::from)?;
    let Some(extract) = summary
        .extract
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        catalog.enqueue_metadata_job("artist", artist.artist_id, RESOLVE_ARTIST_DISCOGS_SEARCH)?;
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("Wikipedia returned no extract"),
        )?;
        return Ok(());
    };

    catalog.save_artist_profile(
        artist.artist_id,
        None,
        Some(extract),
        None,
        Some("wikipedia"),
    )?;
    Ok(())
}

/// Mirror of [`fetch_artist_wikipedia_summary`] for albums. Looks up
/// the release-group `inc=url-rels` block on MusicBrainz, picks a
/// Wikipedia URL, and stores the REST summary `extract` as the album
/// description with `description_source = 'wikipedia'`.
fn fetch_album_wikipedia_summary(
    catalog: &CatalogStore,
    client: &Client,
    album_id: i64,
    last_musicbrainz_request: &mut Option<Instant>,
    last_wikipedia_request: &mut Option<Instant>,
    shutdown_rx: &mpsc::Receiver<()>,
) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    let Some(rg_mbid) = album.musicbrainz_release_group_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album has no MusicBrainz release group ID"),
        )?;
        return Ok(());
    };

    if !wait_for_rate_limit(last_musicbrainz_request, MUSICBRAINZ_DELAY, shutdown_rx) {
        return Ok(());
    }
    let request = client
        .get(format!(
            "https://musicbrainz.org/ws/2/release-group/{rg_mbid}"
        ))
        .query(&[("inc", "url-rels"), ("fmt", "json")]);
    let lookup: MusicBrainzReleaseGroupLookup =
        request_json(client, request, "musicbrainz").map_err(anyhow::Error::from)?;

    let Some(url) = pick_wikipedia_url(&lookup.relations) else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("Album has no Wikipedia URL in MusicBrainz"),
        )?;
        return Ok(());
    };
    let Some(title) = wikipedia_title_from_url(url) else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("Wikipedia URL is unparseable"),
        )?;
        return Ok(());
    };

    if !wait_for_rate_limit(last_wikipedia_request, WIKIPEDIA_DELAY, shutdown_rx) {
        return Ok(());
    }
    let request = client.get(format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{title}"
    ));
    let summary: WikipediaSummary =
        request_json(client, request, "wikipedia").map_err(anyhow::Error::from)?;
    let Some(extract) = summary
        .extract
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("Wikipedia returned no extract"),
        )?;
        return Ok(());
    };

    catalog.save_album_profile(album.album_id, None, Some(extract), Some("wikipedia"))?;
    Ok(())
}

// ----------------------------------------------------------------- //
//                  Discogs (Phase 3, unauthenticated)                //
// ----------------------------------------------------------------- //

/// Discogs artist search by name. Used as the third-tier resolver
/// after MusicBrainz and TheAudioDB miss; persists the Discogs artist
/// id and queues the standard Discogs enrichment chain
/// (`fetch_artist_discogs_profile` + `fetch_artist_discogs_releases`).
fn resolve_artist_discogs_search(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    if artist.discogs_id.is_some() {
        // Already have a Discogs id from a prior path; just (re-)queue
        // the enrichment chain.
        enqueue_artist_discogs_jobs(catalog, artist.artist_id)?;
        return Ok(());
    }

    let request = client
        .get("https://api.discogs.com/database/search")
        .query(&[("type", "artist"), ("q", artist.name.as_str())]);
    let response: DiscogsSearchResponse =
        request_json(client, request, "discogs").map_err(anyhow::Error::from)?;

    let Some(result) = best_discogs_artist_match(&artist.normalized_name, &response.results) else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("No Discogs artist match"),
        )?;
        return Ok(());
    };

    catalog.set_artist_discogs_id(artist.artist_id, &result.id.to_string())?;
    enqueue_artist_discogs_jobs(catalog, artist.artist_id)?;
    Ok(())
}

/// Discogs `/artists/{id}` lookup. Persists the Discogs `profile`
/// blurb as the artist bio (with `bio_source = "discogs"`) and
/// downloads the primary image as the artist photo when one isn't
/// already on hand.
fn fetch_artist_discogs_profile(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(discogs_id) = artist.discogs_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no Discogs ID"),
        )?;
        return Ok(());
    };

    let request = client.get(format!("https://api.discogs.com/artists/{discogs_id}"));
    let response: DiscogsArtist =
        request_json(client, request, "discogs").map_err(anyhow::Error::from)?;

    let bio = response
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|bio| !bio.is_empty());

    let image_url = pick_discogs_image_url(&response.images);
    let photo_asset_id = image_url
        .map(|url| download_external_asset(catalog, client, "artist_photo", "discogs", url))
        .transpose()?;

    if bio.is_none() && photo_asset_id.is_none() {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "missing",
            Some("Discogs artist had no profile or image"),
        )?;
        return Ok(());
    }

    catalog.save_artist_profile(
        artist.artist_id,
        None,
        bio,
        photo_asset_id,
        if bio.is_some() { Some("discogs") } else { None },
    )?;
    Ok(())
}

/// Discogs `/artists/{id}/releases` discography crawler. Pulls every
/// page (capped at [`DISCOGS_RELEASES_MAX_PAGES`]), upserts a
/// `discography_items` row per entry preserving Main / Appearance /
/// etc. roles, and enqueues a `fetch_discogs_thumb` job per row that
/// has a thumbnail URL so the per-thumb downloads can be paced
/// independently.
fn fetch_artist_discogs_releases(
    catalog: &CatalogStore,
    client: &Client,
    artist_id: i64,
    last_discogs_request: &mut Option<Instant>,
    shutdown_rx: &mpsc::Receiver<()>,
) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(discogs_id) = artist.discogs_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no Discogs ID"),
        )?;
        return Ok(());
    };

    let mut saved = 0usize;
    for page in 1..=DISCOGS_RELEASES_MAX_PAGES {
        if shutdown_rx.try_recv().is_ok() {
            return Ok(());
        }
        if !wait_for_rate_limit(last_discogs_request, DISCOGS_DELAY, shutdown_rx) {
            return Ok(());
        }

        let url = format!("https://api.discogs.com/artists/{discogs_id}/releases");
        let per_page = DISCOGS_RELEASES_LIMIT.to_string();
        let page_str = page.to_string();
        let request = client.get(&url).query(&[
            ("sort", "year"),
            ("sort_order", "asc"),
            ("per_page", per_page.as_str()),
            ("page", page_str.as_str()),
        ]);
        let response: DiscogsArtistReleasesResponse =
            request_json(client, request, "discogs").map_err(anyhow::Error::from)?;

        let last_page = response.pagination.pages.unwrap_or(1).max(1) as usize;

        for item in response.releases {
            let title = match item.title.as_deref() {
                Some(t) if !t.trim().is_empty() => t,
                _ => continue,
            };
            let role = item.role.as_deref().unwrap_or("Main");
            let release_type = item
                .format
                .as_deref()
                .map(normalize_discogs_format)
                .unwrap_or_else(|| "album".to_string());
            let year = item
                .year
                .and_then(|y| if y > 0 { Some(y.to_string()) } else { None });

            // For dedup with MB-sourced rows, only set
            // `discogs_master_id` for `type == "master"` entries; pure
            // release entries don't roll up to a master and shouldn't
            // collide with a MB release-group MBID's master id.
            let discogs_master_id = if matches!(item.entity_kind.as_deref(), Some("master")) {
                Some(item.id.to_string())
            } else {
                None
            };

            let id = catalog.upsert_discography_item_full(
                artist.artist_id,
                title,
                year.as_deref(),
                &release_type,
                None, // musicbrainz_release_group_id
                discogs_master_id.as_deref(),
                Some(role),
                item.format.as_deref(),
                Some("discogs"),
                item.thumb.as_deref(),
            )?;
            saved += 1;

            if item.thumb.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                catalog.enqueue_metadata_job("discography_item", id, FETCH_DISCOGS_THUMB)?;
            }
        }

        if page >= last_page {
            break;
        }
    }

    catalog.mark_artist_metadata_checked(
        artist.artist_id,
        if saved == 0 { "missing" } else { "resolved" },
        if saved == 0 {
            Some("No Discogs releases")
        } else {
            None
        },
    )?;
    Ok(())
}

/// Discogs master search by `(artist, release_title)`. Mirror of
/// [`resolve_artist_discogs_search`]: when the album has no
/// `discogs_master_id`, this resolves one and queues the cover-image
/// follow-up.
fn resolve_album_discogs_search(
    catalog: &CatalogStore,
    client: &Client,
    album_id: i64,
) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    if album.discogs_master_id.is_some() {
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_DISCOGS_IMAGE)?;
        return Ok(());
    }

    let request = client
        .get("https://api.discogs.com/database/search")
        .query(&[
            ("type", "master"),
            ("artist", album.artist_name.as_str()),
            ("release_title", album.title.as_str()),
        ]);
    let response: DiscogsSearchResponse =
        request_json(client, request, "discogs").map_err(anyhow::Error::from)?;

    let Some(result) = best_discogs_master_match(&album.normalized_title, &response.results) else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No Discogs master match"),
        )?;
        return Ok(());
    };

    catalog.set_album_discogs_master_id(album.album_id, &result.id.to_string())?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_DISCOGS_IMAGE)?;
    Ok(())
}

/// Discogs `/masters/{id}` lookup. Pulls the primary image and saves
/// it as the album cover via the standard external-asset pipeline.
fn fetch_album_discogs_image(catalog: &CatalogStore, client: &Client, album_id: i64) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    let Some(master_id) = album.discogs_master_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album has no Discogs master ID"),
        )?;
        return Ok(());
    };

    let request = client.get(format!("https://api.discogs.com/masters/{master_id}"));
    let response: DiscogsMaster =
        request_json(client, request, "discogs").map_err(anyhow::Error::from)?;

    let Some(image_url) = pick_discogs_image_url(&response.images) else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("Discogs master had no images"),
        )?;
        return Ok(());
    };

    let asset_id = download_external_asset(catalog, client, "album_cover", "discogs", image_url)?;
    catalog.save_album_cover(album.album_id, asset_id)?;
    Ok(())
}

/// Per-row Discogs thumb downloader. Reads `discography_items.thumb_url`
/// for the given row and downloads it into a `discography_thumb`
/// asset; the row's `cover_asset_id` is then set to the new asset.
/// No-op when the row already has a cover or the URL is missing.
fn fetch_discogs_thumb(catalog: &CatalogStore, client: &Client, item_id: i64) -> Result<()> {
    let Some(target) = catalog.load_discogs_thumb_target(item_id)? else {
        return Ok(());
    };
    let asset_id = download_external_asset(
        catalog,
        client,
        "discography_thumb",
        "discogs",
        &target.source_url,
    )?;
    catalog.set_discography_item_cover(target.item_id, asset_id)?;
    Ok(())
}

/// Pick the best Discogs image for an artist/master payload: prefer
/// `type == "primary"` and fall back to the first available image
/// otherwise. Returns the borrowed `uri` slice.
fn pick_discogs_image_url(images: &[DiscogsImage]) -> Option<&str> {
    images
        .iter()
        .find(|img| img.image_type.as_deref() == Some("primary"))
        .or_else(|| images.iter().find(|img| img.uri.is_some()))
        .and_then(|img| img.uri.as_deref())
        .filter(|url| !url.trim().is_empty())
}

/// Best-match helper for Discogs artist search results. Filters to
/// `entity_type == "artist"` and prefers a normalized title match;
/// falls back to the first artist-typed result otherwise.
fn best_discogs_artist_match<'a>(
    normalized_name: &str,
    results: &'a [DiscogsSearchResult],
) -> Option<&'a DiscogsSearchResult> {
    let is_artist = |r: &&DiscogsSearchResult| {
        r.entity_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("artist"))
    };
    results
        .iter()
        .filter(is_artist)
        .find(|r| {
            r.title.as_deref().map(normalize_external_key).as_deref() == Some(normalized_name)
        })
        .or_else(|| results.iter().find(is_artist))
}

/// Best-match helper for Discogs master search results. Filters to
/// `entity_type == "master"`. Discogs master titles are formatted as
/// "Artist - Album", so we match on the trailing album segment.
fn best_discogs_master_match<'a>(
    normalized_title: &str,
    results: &'a [DiscogsSearchResult],
) -> Option<&'a DiscogsSearchResult> {
    let is_master = |r: &&DiscogsSearchResult| {
        r.entity_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("master"))
    };
    results
        .iter()
        .filter(is_master)
        .find(|r| {
            r.title
                .as_deref()
                .map(discogs_master_album_title)
                .as_deref()
                == Some(normalized_title)
        })
        .or_else(|| results.iter().find(is_master))
}

/// Extract the album-title half of a Discogs master `title` (which
/// arrives as "Artist - Album"), normalized for comparison. Falls back
/// to the whole string when no " - " separator is present.
fn discogs_master_album_title(title: &str) -> String {
    let album = title.split_once(" - ").map(|(_, b)| b).unwrap_or(title);
    normalize_external_key(album)
}

/// Map a Discogs `format` string ("CD, Album", "Vinyl, EP", etc.) to
/// the existing `release_type` vocabulary used by the discography UI:
/// `album` / `ep` / `single` / `compilation`. Defaults to `album`.
fn normalize_discogs_format(format_str: &str) -> String {
    let lower = format_str.to_ascii_lowercase();
    if lower.contains("compilation") {
        "compilation".to_string()
    } else if lower.contains("single") {
        "single".to_string()
    } else if lower.contains(" ep") || lower.contains(",ep") || lower.contains(", ep") {
        "ep".to_string()
    } else {
        "album".to_string()
    }
}

/// Tier-0 enrichment: pull a fully-baked artist record from Lidarr's
/// hosted metadata proxy (`api.lidarr.audio/api/v0.4/artist/<mbid>`).
///
/// One round-trip returns the bio (`overview`), categorized images
/// (`images[]` with `CoverType: Banner|Fanart|Logo|Poster`), full
/// discography (`Albums[]`), and link rels we can reuse to populate
/// `discogs_id`. We persist everything we can and return Ok even when
/// the response is missing fields -- the legacy fallback chain (TADb,
/// Wikipedia, Discogs) is still queued and will fill the remaining
/// gaps on its own.
///
/// Bails (status 'blocked') when the artist has no MBID; Lidarr is
/// MBID-keyed and there's nothing to look up otherwise. The
/// `resolve_artist_musicbrainz` chain handles those cases first.
fn fetch_artist_lidarr(catalog: &CatalogStore, client: &Client, artist_id: i64) -> Result<()> {
    let Some(artist) = catalog.load_metadata_artist(artist_id)? else {
        return Ok(());
    };
    let Some(mbid) = artist.musicbrainz_id.as_deref() else {
        catalog.mark_artist_metadata_checked(
            artist.artist_id,
            "blocked",
            Some("Artist has no MusicBrainz ID for Lidarr lookup"),
        )?;
        return Ok(());
    };

    let url = format!("{LIDARR_API_BASE}/artist/{mbid}");
    let request = client.get(&url);
    let response: LidarrArtist = match request_json(client, request, "lidarr") {
        Ok(value) => value,
        Err(err) if err.status == Some(404) => {
            // Not in Lidarr's cache; let the rest of the chain handle
            // it. Don't mark `missing` because that suppresses
            // downstream re-runs.
            return Ok(());
        }
        Err(err) => return Err(anyhow::Error::from(err)),
    };

    // Bio.
    let overview = response
        .overview
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Pick the best artist image. Order of preference matches what we
    // already use for the artist hero: Poster (square thumb) -> Fanart
    // -> Banner -> Logo. We prefer Lidarr's CDN-cached `Url` over the
    // raw `remoteUrl` because the cdn URL is HEAD-stable.
    let photo_url = pick_lidarr_image(&response.images, &["Poster", "Fanart", "Banner", "Logo"]);
    let photo_asset_id = photo_url
        .map(|url| download_external_asset(catalog, client, "artist_photo", "lidarr", url))
        .transpose()?;

    // Adopt a Discogs id from the link rels if one is present and the
    // artist row doesn't already have one. This unlocks the Discogs
    // profile job downstream without a separate search.
    if artist.discogs_id.is_none()
        && let Some(discogs_id) = pick_discogs_id_from_lidarr_links(&response.links)
    {
        let _ = catalog.set_artist_discogs_id(artist.artist_id, &discogs_id);
    }

    // Persist bio + photo. Tag the source as `lidarr` so we know where
    // it came from; the existing fallback chain re-uses other sources.
    if overview.is_some() || photo_asset_id.is_some() {
        catalog.save_artist_profile(
            artist.artist_id,
            None,
            overview,
            photo_asset_id,
            Some("lidarr"),
        )?;
    } else {
        // Lidarr returned a record but nothing useful -- mark as
        // resolved so we stop spinning, leave bio NULL for fallbacks.
        catalog.mark_artist_metadata_checked(artist.artist_id, "resolved", None)?;
    }

    // Walk the Albums[] payload into `discography_items`. Lidarr's
    // response uses MBID-keyed release group ids, so dedup with
    // existing MB-sourced rows happens via the
    // `(artist_id, musicbrainz_release_group_id)` unique key.
    let mut saved = 0usize;
    for album in &response.albums {
        let Some(rg_mbid) = album.id.as_deref() else {
            continue;
        };
        let title = album.title.as_deref().unwrap_or("").trim();
        if title.is_empty() {
            continue;
        }
        let release_type = album
            .album_type
            .as_deref()
            .map(|t| t.to_ascii_lowercase())
            .unwrap_or_else(|| "album".to_string());
        // Lidarr's `releasedate` is `YYYY-MM-DD`; clamp to year for
        // discography sort.
        let year = album
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .filter(|year| year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_string);
        let _ = catalog.upsert_discography_item_full(
            artist.artist_id,
            title,
            year.as_deref(),
            &release_type,
            Some(rg_mbid),
            None,
            Some("Main"),
            None,
            Some("lidarr"),
            None,
        );
        saved += 1;
    }
    if saved > 0 {
        perf::event(
            "metadata.lidarr.artist_albums",
            format!("artist_id={artist_id} saved={saved}"),
        );
    }

    Ok(())
}

/// Tier-0 enrichment for albums. Pulls overview (used as description)
/// and cover image from `api.lidarr.audio/api/v0.4/album/<mbid>`.
fn fetch_album_lidarr(catalog: &CatalogStore, client: &Client, album_id: i64) -> Result<()> {
    let Some(album) = catalog.load_metadata_album(album_id)? else {
        return Ok(());
    };
    let Some(rg_mbid) = album.musicbrainz_release_group_id.as_deref() else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "blocked",
            Some("Album has no MusicBrainz release group ID for Lidarr lookup"),
        )?;
        return Ok(());
    };

    let url = format!("{LIDARR_API_BASE}/album/{rg_mbid}");
    let request = client.get(&url);
    let response: LidarrAlbum = match request_json(client, request, "lidarr") {
        Ok(value) => value,
        Err(err) if err.status == Some(404) => {
            return Ok(());
        }
        Err(err) => return Err(anyhow::Error::from(err)),
    };

    // Description.
    let overview = response
        .overview
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if overview.is_some() {
        catalog.save_album_profile(album.album_id, None, overview, Some("lidarr"))?;
    }

    // Cover. Prefer the dedicated 'Cover' type, then 'Disc'. Each
    // Lidarr image entry uses the CAA URL on `Url` so we hit Cloudflare
    // first; we fall back to `remoteUrl` if `Url` is empty.
    let cover_url =
        pick_lidarr_image(&response.images, &["Cover", "Disc"]).filter(|url| !url.is_empty());
    if let Some(url) = cover_url {
        let request = client.get(url);
        match request_bytes(client, request, "lidarr") {
            Ok((mime_type, data)) => {
                let _ =
                    catalog.save_album_cover_file(album.album_id, url, mime_type.as_deref(), &data);
            }
            Err(err) => {
                perf::event(
                    "metadata.lidarr.album_cover_error",
                    format!("album_id={album_id} url={url} error={}", err.message),
                );
            }
        }
    }

    Ok(())
}

/// Pick the first image whose `CoverType` matches one of the
/// `preferred_types` (in order). Falls back to the first non-empty
/// `Url` in the list.
fn pick_lidarr_image<'a>(images: &'a [LidarrImage], preferred_types: &[&str]) -> Option<&'a str> {
    for cover_type in preferred_types {
        if let Some(image) = images
            .iter()
            .find(|img| img.cover_type.as_deref() == Some(*cover_type))
            && let Some(url) = image
                .url
                .as_deref()
                .or(image.remote_url.as_deref())
                .filter(|url| !url.trim().is_empty())
        {
            return Some(url);
        }
    }
    images.iter().find_map(|img| {
        img.url
            .as_deref()
            .or(img.remote_url.as_deref())
            .filter(|url| !url.trim().is_empty())
    })
}

/// Lidarr's `links` array is { target, type } objects. Pick the first
/// target whose URL matches a Discogs artist permalink and extract the
/// numeric id.
fn pick_discogs_id_from_lidarr_links(links: &[LidarrLink]) -> Option<String> {
    for link in links {
        let kind = link.kind.as_deref().unwrap_or("");
        if kind != "discogs" {
            continue;
        }
        let target = link.target.as_deref()?;
        // Expected: https://www.discogs.com/artist/<id> (sometimes
        // https://www.discogs.com/artist/<id>-<slug> on older payloads).
        let after = target.split("/artist/").nth(1)?;
        let id_segment = after.split(['/', '-', '?', '#']).next()?;
        if !id_segment.is_empty() && id_segment.chars().all(|c| c.is_ascii_digit()) {
            return Some(id_segment.to_string());
        }
    }
    None
}

fn download_external_asset(
    catalog: &CatalogStore,
    client: &Client,
    kind: &str,
    source: &str,
    source_url: &str,
) -> Result<i64> {
    // The `source` arg is dynamic (caller-provided), but our typed
    // error wants a `&'static str`. Map the known sources we currently
    // pass; fall back to a generic label otherwise.
    let static_source: &'static str = match source {
        "theaudiodb" => "theaudiodb",
        "musicbrainz" => "musicbrainz",
        "coverartarchive" => "coverartarchive",
        "discogs" => "discogs",
        "lidarr" => "lidarr",
        _ => "external_asset",
    };
    let request = client.get(source_url);
    let (mime_type, data) =
        request_bytes(client, request, static_source).map_err(anyhow::Error::from)?;

    catalog.save_external_asset(kind, source, source_url, mime_type.as_deref(), &data)
}

fn best_artist_match<'a>(
    normalized_name: &str,
    artists: &'a [MusicBrainzArtist],
) -> Option<&'a MusicBrainzArtist> {
    artists
        .iter()
        .find(|artist| normalize_external_key(&artist.name) == normalized_name)
        .or_else(|| {
            artists
                .iter()
                .filter(|artist| artist.score.unwrap_or_default() >= 95)
                .max_by_key(|artist| artist.score.unwrap_or_default())
        })
}

fn best_release_group_match<'a>(
    normalized_title: &str,
    release_groups: &'a [MusicBrainzReleaseGroup],
) -> Option<&'a MusicBrainzReleaseGroup> {
    release_groups
        .iter()
        .find(|release_group| normalize_external_key(&release_group.title) == normalized_title)
        .or_else(|| {
            release_groups
                .iter()
                .filter(|release_group| release_group.score.unwrap_or_default() >= 95)
                .max_by_key(|release_group| release_group.score.unwrap_or_default())
        })
}

fn normalize_external_key(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Deserialize)]
struct MusicBrainzArtistSearch {
    #[serde(default)]
    artists: Vec<MusicBrainzArtist>,
}

#[derive(Debug, Deserialize)]
struct MusicBrainzArtist {
    id: String,
    name: String,
    score: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AudioDbArtistResponse {
    artists: Option<Vec<AudioDbArtist>>,
}

#[derive(Debug, Deserialize)]
struct AudioDbArtist {
    #[serde(rename = "idArtist")]
    artist_id: Option<String>,
    #[serde(rename = "strBiographyEN")]
    biography_en: Option<String>,
    #[serde(rename = "strArtistThumb")]
    artist_thumb: Option<String>,
    #[serde(rename = "strArtistFanart")]
    artist_fanart: Option<String>,
    /// Surfaced by both `artist-mb.php` (via the input MBID) and
    /// `search.php?s=<name>`. Used by the audiodb-search resolver to
    /// adopt a MusicBrainz ID when MB itself returned no match.
    #[serde(default, rename = "strMusicBrainzID")]
    musicbrainz_id: Option<String>,
}

/// Wrapper for `album-mb.php`. TheAudioDB returns `{"album": [...]}`
/// rather than `{"albums": [...]}`, hence the singular field name.
#[derive(Debug, Deserialize)]
struct AudioDbAlbumResponse {
    album: Option<Vec<AudioDbAlbum>>,
}

#[derive(Debug, Deserialize)]
struct AudioDbAlbum {
    #[serde(rename = "idAlbum")]
    album_id: Option<String>,
    #[serde(rename = "strDescriptionEN")]
    description_en: Option<String>,
}

/// Wrapper for `searchalbum.php?s=<artist>&a=<album>`. Like
/// [`AudioDbAlbumResponse`], the field is singular `album`. We only
/// need the release-group MBID and album id from the entries, so we
/// keep the row struct minimal.
#[derive(Debug, Deserialize)]
struct AudioDbAlbumSearchResponse {
    album: Option<Vec<AudioDbAlbumSearchResult>>,
}

#[derive(Debug, Deserialize)]
struct AudioDbAlbumSearchResult {
    #[serde(default, rename = "idAlbum")]
    #[allow(dead_code)]
    album_id: Option<String>,
    #[serde(default, rename = "strMusicBrainzID")]
    musicbrainz_id: Option<String>,
}

/// MusicBrainz `inc=url-rels` lookup payload for an artist. We only
/// care about the URL relationships, so the rest of the response is
/// dropped via `serde(default)`.
#[derive(Debug, Deserialize)]
struct MusicBrainzArtistLookup {
    #[serde(default)]
    relations: Vec<MusicBrainzUrlRelation>,
}

/// Same shape as [`MusicBrainzArtistLookup`] for release groups.
/// MusicBrainz returns the `relations` array under the same field
/// name regardless of entity, so the wrappers exist purely for
/// type-level disambiguation.
#[derive(Debug, Deserialize)]
struct MusicBrainzReleaseGroupLookup {
    #[serde(default)]
    relations: Vec<MusicBrainzUrlRelation>,
}

#[derive(Debug, Deserialize)]
struct MusicBrainzUrlRelation {
    #[serde(rename = "type")]
    relation_type: Option<String>,
    url: MusicBrainzUrlResource,
}

#[derive(Debug, Deserialize)]
struct MusicBrainzUrlResource {
    resource: String,
}

/// Wikipedia REST `page/summary` payload. Only the `extract` field is
/// consumed; the other JSON members (titles, thumbnail metadata, etc.)
/// are intentionally ignored.
#[derive(Debug, Deserialize)]
struct WikipediaSummary {
    extract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MusicBrainzReleaseGroupResponse {
    #[serde(default, rename = "release-groups")]
    release_groups: Vec<MusicBrainzReleaseGroup>,
}

#[derive(Debug, Deserialize)]
struct MusicBrainzReleaseGroup {
    id: String,
    title: String,
    score: Option<u32>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
}

/// Discogs `database/search` response. We use the same wrapper for
/// both `type=artist` and `type=master` searches; the per-result
/// `entity_type` field disambiguates.
#[derive(Debug, Deserialize)]
struct DiscogsSearchResponse {
    #[serde(default)]
    results: Vec<DiscogsSearchResult>,
}

#[derive(Debug, Deserialize)]
struct DiscogsSearchResult {
    id: i64,
    title: Option<String>,
    /// `"artist"`, `"master"`, or `"release"`. For artist results,
    /// `title` is the artist name; for masters it's "Artist - Album".
    #[serde(rename = "type")]
    entity_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscogsArtist {
    profile: Option<String>,
    #[serde(default)]
    images: Vec<DiscogsImage>,
}

#[derive(Debug, Deserialize)]
struct DiscogsImage {
    uri: Option<String>,
    /// `"primary"` or `"secondary"`. Primary is the main artwork on
    /// Discogs (artist headshot / master cover); secondary entries are
    /// alternate shots.
    #[serde(rename = "type")]
    image_type: Option<String>,
    #[allow(dead_code)]
    height: Option<i64>,
    #[allow(dead_code)]
    width: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DiscogsMaster {
    #[serde(default)]
    images: Vec<DiscogsImage>,
}

#[derive(Debug, Deserialize)]
struct DiscogsArtistReleasesResponse {
    #[serde(default)]
    pagination: DiscogsPagination,
    #[serde(default)]
    releases: Vec<DiscogsReleaseItem>,
}

#[derive(Debug, Deserialize, Default)]
struct DiscogsPagination {
    #[allow(dead_code)]
    page: Option<u32>,
    pages: Option<u32>,
    #[allow(dead_code)]
    items: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct DiscogsReleaseItem {
    id: i64,
    title: Option<String>,
    year: Option<i64>,
    /// Comma-separated format string like "CD, Album" or
    /// "Vinyl, LP, Compilation". The first comma-separated token is
    /// the medium; later tokens describe the kind. We keep the raw
    /// string in `discography_items.format` and derive
    /// `release_type` via [`normalize_discogs_format`].
    format: Option<String>,
    /// `"Main"`, `"Appearance"`, `"Composed By"`, etc. Stored verbatim
    /// in `discography_items.role`.
    role: Option<String>,
    thumb: Option<String>,
    /// `"master"` or `"release"`. For dedup with MB-sourced rows we
    /// only treat `"master"` entries as having a stable `discogs_master_id`.
    #[serde(rename = "type")]
    entity_kind: Option<String>,
}

impl MusicBrainzReleaseGroup {
    fn year(&self) -> Option<String> {
        self.first_release_date
            .as_deref()
            .map(|date| {
                date.chars()
                    .filter(|ch| ch.is_ascii_digit())
                    .take(4)
                    .collect()
            })
            .filter(|year: &String| year.len() == 4)
    }
}

// ---------------------------------------------------------------------
// Lidarr metadata proxy DTOs (api.lidarr.audio/api/v0.4)
//
// Field names match Lidarr's wire format verbatim. The proxy mixes
// PascalCase (`Albums`) with lowercase (`overview`, `images`,
// `artistname`) -- we use `#[serde(rename = ...)]` for the PascalCase
// keys and map the lowercase ones via `rename_all = "lowercase"`
// where it covers everything cleanly. Otherwise spelled out
// individually for clarity.
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LidarrArtist {
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    images: Vec<LidarrImage>,
    #[serde(default, rename = "Albums")]
    albums: Vec<LidarrAlbumSummary>,
    #[serde(default)]
    links: Vec<LidarrLink>,
}

#[derive(Debug, Deserialize)]
struct LidarrAlbum {
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    images: Vec<LidarrImage>,
}

#[derive(Debug, Deserialize)]
struct LidarrImage {
    #[serde(rename = "CoverType")]
    cover_type: Option<String>,
    #[serde(rename = "Url")]
    url: Option<String>,
    #[serde(rename = "remoteUrl")]
    remote_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LidarrAlbumSummary {
    #[serde(rename = "Id")]
    id: Option<String>,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "Type")]
    album_type: Option<String>,
    #[serde(rename = "ReleaseDate")]
    release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LidarrLink {
    #[serde(default)]
    target: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_exact_normalized_artist_match() {
        let artists = vec![
            MusicBrainzArtist {
                id: "wrong".to_string(),
                name: "A Different Artist".to_string(),
                score: Some(100),
            },
            MusicBrainzArtist {
                id: "right".to_string(),
                name: "Brian   Eno".to_string(),
                score: Some(88),
            },
        ];

        let match_ = best_artist_match("brian eno", &artists).unwrap();
        assert_eq!(match_.id, "right");
    }

    #[test]
    fn extracts_wikipedia_title_from_url() {
        assert_eq!(
            wikipedia_title_from_url("https://en.wikipedia.org/wiki/Brian_Eno"),
            Some("Brian_Eno")
        );
        assert_eq!(
            wikipedia_title_from_url("http://en.wikipedia.org/wiki/Kid_A?foo=bar"),
            Some("Kid_A")
        );
        assert_eq!(
            wikipedia_title_from_url(
                "https://en.wikipedia.org/wiki/A_Hard_Day%27s_Night#Reception"
            ),
            Some("A_Hard_Day%27s_Night")
        );
        assert_eq!(wikipedia_title_from_url("https://example.com/foo"), None);
        assert_eq!(wikipedia_title_from_url("https://en.wikipedia.org/"), None);
        assert_eq!(
            wikipedia_title_from_url("https://en.wikipedia.org/wiki/"),
            None
        );
    }

    #[test]
    fn picks_english_wikipedia_url_first() {
        let rels = vec![
            MusicBrainzUrlRelation {
                relation_type: Some("wikipedia".to_string()),
                url: MusicBrainzUrlResource {
                    resource: "https://de.wikipedia.org/wiki/Brian_Eno".to_string(),
                },
            },
            MusicBrainzUrlRelation {
                relation_type: Some("wikipedia".to_string()),
                url: MusicBrainzUrlResource {
                    resource: "https://en.wikipedia.org/wiki/Brian_Eno".to_string(),
                },
            },
            MusicBrainzUrlRelation {
                relation_type: Some("homepage".to_string()),
                url: MusicBrainzUrlResource {
                    resource: "https://example.com".to_string(),
                },
            },
        ];
        assert_eq!(
            pick_wikipedia_url(&rels),
            Some("https://en.wikipedia.org/wiki/Brian_Eno")
        );
    }

    #[test]
    fn falls_back_to_non_english_wikipedia_url() {
        let rels = vec![MusicBrainzUrlRelation {
            relation_type: Some("wikipedia".to_string()),
            url: MusicBrainzUrlResource {
                resource: "https://ja.wikipedia.org/wiki/Some_Title".to_string(),
            },
        }];
        assert_eq!(
            pick_wikipedia_url(&rels),
            Some("https://ja.wikipedia.org/wiki/Some_Title")
        );
    }
}
