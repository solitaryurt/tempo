use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use reqwest::{StatusCode, blocking::Client, header::CONTENT_TYPE};
use serde::Deserialize;

use crate::{catalog::CatalogStore, perf};

const MUSICBRAINZ_ARTIST_RESOLVE: &str = "resolve_artist_musicbrainz";
const FETCH_ARTIST_PROFILE: &str = "fetch_artist_profile";
const FETCH_ARTIST_DISCOGRAPHY: &str = "fetch_artist_discography";
const RESOLVE_ALBUM_MUSICBRAINZ: &str = "resolve_album_musicbrainz";
const FETCH_ALBUM_COVER: &str = "fetch_album_cover";
const SUPPORTED_JOB_TYPES: &[&str] = &[
    MUSICBRAINZ_ARTIST_RESOLVE,
    FETCH_ARTIST_PROFILE,
    FETCH_ARTIST_DISCOGRAPHY,
    RESOLVE_ALBUM_MUSICBRAINZ,
    FETCH_ALBUM_COVER,
];
const WORKER_IDLE_DELAY: Duration = Duration::from_secs(10);
const MUSICBRAINZ_DELAY: Duration = Duration::from_secs(1);
const THEAUDIODB_DELAY: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
const MUSICBRAINZ_RELEASE_GROUP_LIMIT: usize = 100;
const MUSICBRAINZ_RELEASE_GROUP_MAX_PAGES: usize = 5;
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

    match catalog.reset_stale_metadata_jobs() {
        Ok(count) if count > 0 => {
            perf::event("metadata.worker.reset_stale", format!("jobs={count}"));
        }
        Ok(_) => {}
        Err(error) => perf::event("metadata.worker.reset_stale_error", format!("{error:#}")),
    }

    match catalog.enqueue_missing_online_metadata_jobs() {
        Ok(count) => perf::event("metadata.worker.backfill", format!("jobs={count}")),
        Err(error) => perf::event("metadata.worker.backfill_error", format!("{error:#}")),
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
                if let Err(error) = catalog.fail_metadata_job(job.job_id, &message) {
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

    let response = client
        .get("https://musicbrainz.org/ws/2/artist")
        .query(&[
            ("query", artist.name.as_str()),
            ("fmt", "json"),
            ("limit", "10"),
        ])
        .send()
        .context("failed to query MusicBrainz artist search")?
        .error_for_status()
        .context("MusicBrainz artist search returned an error")?
        .json::<MusicBrainzArtistSearch>()
        .context("failed to parse MusicBrainz artist search response")?;

    let Some(match_) = best_artist_match(&artist.normalized_name, &response.artists) else {
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
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_PROFILE)?;
    catalog.enqueue_metadata_job("artist", artist_id, FETCH_ARTIST_DISCOGRAPHY)?;
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

    let response = client
        .get("https://www.theaudiodb.com/api/v1/json/123/artist-mb.php")
        .query(&[("i", musicbrainz_id)])
        .send()
        .context("failed to query TheAudioDB artist profile")?
        .error_for_status()
        .context("TheAudioDB artist profile returned an error")?
        .json::<AudioDbArtistResponse>()
        .context("failed to parse TheAudioDB artist profile response")?;

    let Some(profile) = response.artists.into_iter().flatten().next() else {
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

    catalog.save_artist_profile(
        artist.artist_id,
        profile.artist_id.as_deref(),
        profile
            .biography_en
            .as_deref()
            .filter(|bio| !bio.trim().is_empty()),
        photo_asset_id,
    )?;
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
        let response = client
            .get("https://musicbrainz.org/ws/2/release-group")
            .query(&query)
            .send()
            .context("failed to query MusicBrainz release groups")?
            .error_for_status()
            .context("MusicBrainz release groups returned an error")?
            .json::<MusicBrainzReleaseGroupResponse>()
            .context("failed to parse MusicBrainz release group response")?;

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
        catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
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
    let response = client
        .get("https://musicbrainz.org/ws/2/release-group")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "10")])
        .send()
        .context("failed to query MusicBrainz album release group")?
        .error_for_status()
        .context("MusicBrainz album release group returned an error")?
        .json::<MusicBrainzReleaseGroupResponse>()
        .context("failed to parse MusicBrainz album release group response")?;

    let Some(match_) = best_release_group_match(&album.normalized_title, &response.release_groups)
    else {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No MusicBrainz release group match"),
        )?;
        return Ok(());
    };

    catalog.resolve_album_musicbrainz_release_group_id(album.album_id, &match_.id)?;
    catalog.enqueue_metadata_job("album", album.album_id, FETCH_ALBUM_COVER)?;
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
    let response = client
        .get(&url)
        .send()
        .context("failed to query Cover Art Archive album cover")?;
    if response.status() == StatusCode::NOT_FOUND {
        catalog.mark_album_metadata_checked(
            album.album_id,
            "missing",
            Some("No Cover Art Archive release group cover"),
        )?;
        return Ok(());
    }
    let response = response
        .error_for_status()
        .context("Cover Art Archive album cover returned an error")?;
    let mime_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let data = response
        .bytes()
        .context("failed to read Cover Art Archive album cover bytes")?;

    catalog.save_album_cover_file(album.album_id, &url, mime_type.as_deref(), &data)?;
    Ok(())
}

fn download_external_asset(
    catalog: &CatalogStore,
    client: &Client,
    kind: &str,
    source: &str,
    source_url: &str,
) -> Result<i64> {
    let response = client
        .get(source_url)
        .send()
        .with_context(|| format!("failed to download {source} asset"))?
        .error_for_status()
        .with_context(|| format!("{source} asset download returned an error"))?;
    let mime_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let data = response
        .bytes()
        .with_context(|| format!("failed to read {source} asset bytes"))?;

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
}
