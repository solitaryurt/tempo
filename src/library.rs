use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use crate::{
    catalog::{CatalogFileFingerprint, CatalogStore, CatalogTrack},
    perf,
};
use anyhow::{Context, Result, anyhow};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    read_from_path,
    tag::{Accessor, ItemKey, ItemValue, Tag},
};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "wave", "wv",
];
const ARTWORK_STEMS: &[&str] = &["cover", "folder", "front", "album"];
const ARTWORK_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "jpeg", "jpg", "png", "tif", "tiff", "webp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artwork {
    Embedded {
        mime_type: Option<String>,
        data: Vec<u8>,
    },
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<String>,
    pub duration: Duration,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bitrate: Option<u32>,
    pub file_size: u64,
    pub date_added: SystemTime,
    pub modified: Option<SystemTime>,
    pub artwork: Option<Artwork>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanReport {
    pub tracks: Vec<Track>,
    pub errors: Vec<IndexingError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryEvent {
    ScanStarted,
    ScanProgress(ScanProgress),
    TracksIndexed(Vec<Track>),
    TrackRemoved(PathBuf),
    TracksRemoved(Vec<PathBuf>),
    ScanError(IndexingError),
    ScanFinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanProgress {
    pub discovered: usize,
    pub indexed: usize,
    pub errors: usize,
}

#[derive(Debug, Clone)]
pub struct IndexerOptions {
    pub ignore_hidden: bool,
    pub debounce: Duration,
    pub batch_size: usize,
}

impl Default for IndexerOptions {
    fn default() -> Self {
        Self {
            ignore_hidden: true,
            debounce: Duration::from_millis(250),
            batch_size: 128,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LibraryIndexer {
    roots: Vec<PathBuf>,
    options: IndexerOptions,
    catalog: Option<CatalogStore>,
}

impl LibraryIndexer {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::with_options(roots, IndexerOptions::default())
    }

    pub fn with_options(roots: impl IntoIterator<Item = PathBuf>, options: IndexerOptions) -> Self {
        Self {
            roots: roots.into_iter().collect(),
            options,
            catalog: None,
        }
    }

    pub fn with_catalog(mut self, catalog: CatalogStore) -> Self {
        self.catalog = Some(catalog);
        self
    }

    pub fn scan(&self) -> ScanReport {
        let _span = perf::span("library.scan", format!("roots={}", self.roots.len()));
        let mut report = ScanReport::default();

        for root in &self.roots {
            for entry in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| should_descend(entry, self.options.ignore_hidden))
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        report.errors.push(IndexingError {
                            path: error.path().unwrap_or(root).to_path_buf(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };

                let path = entry.path();
                if !entry.file_type().is_file() || !is_supported_audio_file(path) {
                    continue;
                }

                match index_audio_file(path) {
                    Ok(track) => report.tracks.push(track),
                    Err(error) => report.errors.push(IndexingError {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    }),
                }
            }
        }

        report
            .tracks
            .sort_by(|left, right| left.path.cmp(&right.path));
        report
    }

    pub fn scan_with_events(&self, events: &mpsc::Sender<LibraryEvent>) -> ScanReport {
        let _span = perf::span(
            "library.scan_with_events",
            format!("roots={}", self.roots.len()),
        );
        self.scan_with_events_until(events, || false).0
    }

    fn scan_with_events_until(
        &self,
        events: &mpsc::Sender<LibraryEvent>,
        mut should_stop: impl FnMut() -> bool,
    ) -> (ScanReport, bool) {
        let _span = perf::span(
            "library.scan_with_events_until",
            format!(
                "roots={} batch_size={}",
                self.roots.len(),
                self.options.batch_size
            ),
        );
        let mut report = ScanReport::default();
        let mut progress = ScanProgress::default();
        let batch_size = self.options.batch_size.max(1);

        let _ = events.send(LibraryEvent::ScanStarted);
        let scan_id =
            self.catalog
                .as_ref()
                .and_then(|catalog| match catalog.begin_scan(&self.roots) {
                    Ok(scan_id) => Some(scan_id),
                    Err(error) => {
                        send_error(
                            events,
                            PathBuf::new(),
                            format!("failed to start catalog scan: {error:#}"),
                        );
                        None
                    }
                });
        let cached_tracks = self.catalog.as_ref().and_then(|catalog| {
            match perf::time_result(
                "library.scan.load_track_fingerprints",
                format!("roots={}", self.roots.len()),
                || catalog.load_track_fingerprints(&self.roots),
            ) {
                Ok(tracks) => Some(tracks),
                Err(error) => {
                    send_error(
                        events,
                        PathBuf::new(),
                        format!("failed to load catalog fingerprints: {error:#}"),
                    );
                    None
                }
            }
        });
        let mut cached_seen_paths = Vec::new();

        // Buffer of paths whose fingerprint did NOT match the catalog --
        // these need a fresh `index_audio_file` parse. Filled by the
        // walker loop; flushed (in parallel) when it grows past
        // `batch_size`.
        let mut pending_parse: Vec<PathBuf> = Vec::with_capacity(batch_size);

        // Helper: parse a batch of paths in parallel using rayon, run
        // the batched DB upsert, and emit events. Returns whether any
        // tracks were actually emitted.
        let flush_pending_parse = |pending: &mut Vec<PathBuf>,
                                   progress: &mut ScanProgress,
                                   report: &mut ScanReport,
                                   events: &mpsc::Sender<LibraryEvent>|
         -> bool {
            if pending.is_empty() {
                return false;
            }
            let parse_count = pending.len();
            let parse_start = std::time::Instant::now();
            // Rayon parallel parse: lofty's `read_from_path` is CPU-bound
            // (FLAC/MP3 header parse) and easily saturates 4-8 cores on
            // a cold scan. Each worker uses Result<Track, IndexingError>
            // so failures don't poison the batch.
            let results: Vec<Result<Track, IndexingError>> = pending
                .par_drain(..)
                .map(|path| {
                    index_audio_file(&path).map_err(|error| IndexingError {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    })
                })
                .collect();
            perf::log_duration(
                "library.scan.parse_batch_parallel",
                parse_start.elapsed(),
                format!("count={parse_count}"),
            );

            let mut batch_tracks = Vec::with_capacity(parse_count);
            for result in results {
                match result {
                    Ok(track) => {
                        progress.indexed += 1;
                        report.tracks.push(track.clone());
                        batch_tracks.push(track);
                    }
                    Err(error) => {
                        progress.errors += 1;
                        report.errors.push(error.clone());
                        let _ = events.send(LibraryEvent::ScanError(error));
                    }
                }
            }

            if batch_tracks.is_empty() {
                let _ = events.send(LibraryEvent::ScanProgress(*progress));
                return false;
            }

            if let Some(catalog) = &self.catalog
                && let Err(error) = catalog.upsert_tracks_batch(&batch_tracks, scan_id)
            {
                send_error(
                    events,
                    PathBuf::new(),
                    format!("failed to cache indexed batch: {error:#}"),
                );
            }
            perf::event(
                "library.scan.emit_batch",
                format!(
                    "kind=indexed batch={} indexed={} discovered={} errors={}",
                    batch_tracks.len(),
                    progress.indexed,
                    progress.discovered,
                    progress.errors
                ),
            );
            // Strip embedded artwork bytes before sending the event:
            // catalog already persisted them, and the UI reads
            // `artwork_path` from the catalog -- shipping the raw bytes
            // through the channel just bloats memory for no benefit.
            let mut event_tracks = batch_tracks;
            for track in &mut event_tracks {
                if matches!(track.artwork, Some(Artwork::Embedded { .. })) {
                    track.artwork = None;
                }
            }
            let _ = events.send(LibraryEvent::TracksIndexed(event_tracks));
            let _ = events.send(LibraryEvent::ScanProgress(*progress));
            true
        };

        for root in &self.roots {
            for entry in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| should_descend(entry, self.options.ignore_hidden))
            {
                if should_stop() {
                    return (report, false);
                }

                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        progress.errors += 1;
                        let indexing_error = IndexingError {
                            path: error.path().unwrap_or(root).to_path_buf(),
                            message: error.to_string(),
                        };
                        report.errors.push(indexing_error.clone());
                        let _ = events.send(LibraryEvent::ScanError(indexing_error));
                        let _ = events.send(LibraryEvent::ScanProgress(progress));
                        continue;
                    }
                };

                let path = entry.path();
                if !entry.file_type().is_file() || !is_supported_audio_file(path) {
                    continue;
                }

                progress.discovered += 1;

                if let Some((stored_fingerprint, cached_track)) =
                    cached_tracks.as_ref().and_then(|tracks| tracks.get(path))
                    && CatalogFileFingerprint::from_path(path).is_some_and(|current_fingerprint| {
                        stored_fingerprint.matches(&current_fingerprint)
                    })
                {
                    progress.indexed += 1;
                    report.tracks.push(track_from_catalog(cached_track.clone()));
                    cached_seen_paths.push(path.to_path_buf());

                    if progress.indexed % batch_size == 0 {
                        perf::event(
                            "library.scan.cached_progress",
                            format!(
                                "indexed={} discovered={} errors={}",
                                progress.indexed, progress.discovered, progress.errors
                            ),
                        );
                        let _ = events.send(LibraryEvent::ScanProgress(progress));
                    }

                    continue;
                }

                pending_parse.push(path.to_path_buf());
                if pending_parse.len() >= batch_size {
                    flush_pending_parse(&mut pending_parse, &mut progress, &mut report, events);
                }
            }
        }

        // Final flush: any remainder parse + commit.
        flush_pending_parse(&mut pending_parse, &mut progress, &mut report, events);

        report
            .tracks
            .sort_by(|left, right| left.path.cmp(&right.path));
        if let (Some(catalog), Some(scan_id)) = (&self.catalog, scan_id) {
            if let Err(error) = catalog.mark_paths_seen(scan_id, &cached_seen_paths) {
                send_error(
                    events,
                    PathBuf::new(),
                    format!("failed to mark cached files as seen: {error:#}"),
                );
            }
            match catalog.finish_scan(scan_id, &self.roots) {
                Ok(removed_paths) => {
                    if !removed_paths.is_empty() {
                        let _ = events.send(LibraryEvent::TracksRemoved(removed_paths));
                    }
                }
                Err(error) => {
                    send_error(
                        events,
                        PathBuf::new(),
                        format!("failed to finish catalog scan: {error:#}"),
                    );
                }
            }
        }
        let _ = events.send(LibraryEvent::ScanProgress(progress));
        let _ = events.send(LibraryEvent::ScanFinished);
        perf::event(
            "library.scan.finished",
            format!(
                "completed=true discovered={} indexed={} errors={}",
                progress.discovered, progress.indexed, progress.errors
            ),
        );
        (report, true)
    }

    pub fn start_watching(self, events: mpsc::Sender<LibraryEvent>) -> Result<LibraryWatcher> {
        let _span = perf::span(
            "library.start_watching",
            format!("roots={}", self.roots.len()),
        );
        let roots = self.roots.clone();
        let options = self.options.clone();
        let catalog = self.catalog.clone();
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("tempo-library-watcher".to_string())
            .spawn(move || run_watcher(roots, options, catalog, events, shutdown_rx))
            .context("failed to spawn library watcher thread")?;

        Ok(LibraryWatcher {
            shutdown_tx,
            handle: Some(handle),
        })
    }
}

#[derive(Debug)]
pub struct LibraryWatcher {
    shutdown_tx: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl LibraryWatcher {
    pub fn stop(mut self) {
        let _span = perf::span("library.watcher.stop", "");
        let _ = self.shutdown_tx.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LibraryWatcher {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

pub fn is_supported_audio_file(path: impl AsRef<Path>) -> bool {
    let Some(extension) = path
        .as_ref()
        .extension()
        .and_then(|extension| extension.to_str())
    else {
        return false;
    };

    AUDIO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
}

pub fn index_audio_file(path: impl AsRef<Path>) -> Result<Track> {
    let path = path.as_ref();
    let _span = perf::slow_span(
        "library.index_audio_file",
        Duration::from_millis(25),
        format!("path={}", path.display()),
    );
    if !is_supported_audio_file(path) {
        return Err(anyhow!("unsupported audio extension"));
    }

    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    let tagged_file = read_from_path(path)
        .with_context(|| format!("failed to read audio metadata from {}", path.display()))?;
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    let properties = tagged_file.properties();
    let artwork = find_artwork(path, tag);

    let filename_title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("Untitled");

    let year = tag
        .and_then(|tag| tag.date())
        .map(|date| date.to_string())
        .filter(|date| !date.trim().is_empty());

    Ok(Track {
        path: path.to_path_buf(),
        title: tag
            .and_then(|tag| tag.title())
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| filename_title.to_string()),
        artist: tag
            .and_then(|tag| tag.artist())
            .map(|artist| artist.trim().to_string())
            .filter(|artist| !artist.is_empty())
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album: tag
            .and_then(|tag| tag.album())
            .map(|album| album.trim().to_string())
            .filter(|album| !album.is_empty())
            .unwrap_or_else(|| "Unknown Album".to_string()),
        genre: tag.and_then(genre_from_tag),
        track_number: tag.and_then(track_number_from_tag),
        year,
        duration: properties.duration(),
        codec: codec_label(path),
        sample_rate: properties.sample_rate(),
        channels: properties.channels(),
        bitrate: properties
            .audio_bitrate()
            .or_else(|| properties.overall_bitrate()),
        file_size: metadata.len(),
        date_added: SystemTime::now(),
        modified: metadata.modified().ok(),
        artwork,
    })
}

fn track_from_catalog(track: CatalogTrack) -> Track {
    Track {
        path: track.path,
        title: track.title,
        artist: track.artist,
        album: track.album,
        genre: track.genre,
        track_number: track.track_number,
        year: track.year,
        date_added: track.date_added,
        duration: track.duration,
        codec: track.codec,
        sample_rate: None,
        channels: None,
        bitrate: track.bitrate,
        file_size: track.file_size,
        modified: None,
        artwork: track.artwork_path.map(Artwork::File),
    }
}

fn genre_from_tag(tag: &Tag) -> Option<String> {
    tag.genre()
        .map(|genre| genre.trim().to_string())
        .filter(|genre| !genre.is_empty())
}

fn track_number_from_tag(tag: &Tag) -> Option<u32> {
    tag.items()
        .filter(|item| item.key() == ItemKey::TrackNumber)
        .filter_map(|item| match item.value() {
            ItemValue::Text(value) | ItemValue::Locator(value) => parse_track_number(value),
            _ => None,
        })
        .next()
        .or_else(|| tag.track())
}

fn parse_track_number(value: &str) -> Option<u32> {
    let digits = value
        .trim()
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();

    digits.parse().ok().filter(|track| *track > 0)
}

fn find_artwork(path: &Path, tag: Option<&Tag>) -> Option<Artwork> {
    tag.and_then(embedded_artwork)
        .or_else(|| find_folder_artwork(path).map(Artwork::File))
}

fn embedded_artwork(tag: &Tag) -> Option<Artwork> {
    let picture = tag
        .get_picture_type(PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;

    if picture.data().is_empty() {
        return None;
    }

    Some(Artwork::Embedded {
        mime_type: picture
            .mime_type()
            .map(|mime_type| mime_type.as_str().to_string()),
        data: picture.data().to_vec(),
    })
}

fn find_folder_artwork(audio_path: &Path) -> Option<PathBuf> {
    let directory = audio_path.parent()?;
    let mut candidates = fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }

            let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();

            (ARTWORK_STEMS.contains(&stem.as_str())
                && ARTWORK_EXTENSIONS.contains(&extension.as_str()))
            .then_some(path)
        })
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.into_iter().next()
}

fn run_watcher(
    roots: Vec<PathBuf>,
    options: IndexerOptions,
    catalog: Option<CatalogStore>,
    events: mpsc::Sender<LibraryEvent>,
    shutdown_rx: mpsc::Receiver<()>,
) {
    let _span = perf::span("library.run_watcher", format!("roots={}", roots.len()));
    let mut indexer = LibraryIndexer::with_options(roots.clone(), options.clone());
    indexer.catalog = catalog.clone();
    let mut shutdown_requested = false;
    let (_, completed_scan) = indexer.scan_with_events_until(&events, || {
        if shutdown_requested {
            return true;
        }

        match shutdown_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                shutdown_requested = true;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    });

    if !completed_scan || shutdown_requested {
        return;
    }

    let (notify_tx, notify_rx) = mpsc::channel();
    let watcher_result = RecommendedWatcher::new(
        move |result| {
            let _ = notify_tx.send(result);
        },
        Config::default(),
    );

    let mut watcher = match watcher_result {
        Ok(watcher) => watcher,
        Err(error) => {
            send_error(&events, PathBuf::new(), error.to_string());
            return;
        }
    };

    for root in &roots {
        if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
            send_error(&events, root.clone(), error.to_string());
        }
    }

    let mut pending: HashMap<PathBuf, PendingPath> = HashMap::new();

    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        match notify_rx.recv_timeout(options.debounce) {
            Ok(Ok(event)) => {
                record_notify_event(event, &roots, options.ignore_hidden, &mut pending)
            }
            Ok(Err(error)) => send_error(&events, PathBuf::new(), error.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                flush_pending(&events, catalog.as_ref(), &mut pending)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPath {
    Index,
    Remove,
}

fn record_notify_event(
    event: Event,
    roots: &[PathBuf],
    ignore_hidden: bool,
    pending: &mut HashMap<PathBuf, PendingPath>,
) {
    let action = match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
            PendingPath::Index
        }
        EventKind::Remove(_) => PendingPath::Remove,
        EventKind::Access(_) => return,
    };

    for path in event.paths {
        if is_hidden_path(&path, roots, ignore_hidden) {
            continue;
        }

        if action != PendingPath::Remove && !is_supported_audio_file(&path) {
            continue;
        }

        pending.insert(path, action);
    }
}

fn flush_pending(
    events: &mpsc::Sender<LibraryEvent>,
    catalog: Option<&CatalogStore>,
    pending: &mut HashMap<PathBuf, PendingPath>,
) {
    let pending_paths = std::mem::take(pending);
    let mut indexed = Vec::new();
    let mut removed = HashSet::new();

    for (path, action) in pending_paths {
        match action {
            PendingPath::Index if path.exists() => match index_audio_file(&path) {
                Ok(track) => {
                    if let Some(catalog) = catalog
                        && let Err(error) = catalog.upsert_track(&track, None)
                    {
                        send_error(
                            events,
                            path.clone(),
                            format!("failed to cache indexed metadata: {error:#}"),
                        );
                    }
                    indexed.push(track);
                }
                Err(error) => send_error(events, path, error.to_string()),
            },
            PendingPath::Index | PendingPath::Remove => {
                if is_supported_audio_file(&path) {
                    if let Some(catalog) = catalog
                        && let Err(error) = catalog.mark_file_removed(&path)
                    {
                        send_error(
                            events,
                            path.clone(),
                            format!("failed to mark removed file in catalog: {error:#}"),
                        );
                    }
                    removed.insert(path);
                } else if let Some(catalog) = catalog {
                    match catalog.mark_folder_removed(&path) {
                        Ok(paths) => removed.extend(paths),
                        Err(error) => send_error(
                            events,
                            path.clone(),
                            format!("failed to mark removed folder in catalog: {error:#}"),
                        ),
                    }
                }
            }
        }
    }

    if !indexed.is_empty() {
        // Same artwork-bytes stripping as the cold scan path: catalog
        // already persisted embedded artwork to disk, so we don't need
        // to ship the raw bytes through the event channel.
        let mut event_tracks = indexed;
        for track in &mut event_tracks {
            if matches!(track.artwork, Some(Artwork::Embedded { .. })) {
                track.artwork = None;
            }
        }
        let _ = events.send(LibraryEvent::TracksIndexed(event_tracks));
    }

    if removed.len() == 1 {
        if let Some(path) = removed.into_iter().next() {
            let _ = events.send(LibraryEvent::TrackRemoved(path));
        }
    } else if !removed.is_empty() {
        let mut removed = removed.into_iter().collect::<Vec<_>>();
        removed.sort();
        let _ = events.send(LibraryEvent::TracksRemoved(removed));
    }
}

fn send_error(events: &mpsc::Sender<LibraryEvent>, path: PathBuf, message: String) {
    let _ = events.send(LibraryEvent::ScanError(IndexingError { path, message }));
}

fn should_descend(entry: &DirEntry, ignore_hidden: bool) -> bool {
    if entry.depth() == 0 {
        return true;
    }

    !is_hidden_name(entry.file_name().to_str(), ignore_hidden)
}

fn is_hidden_path(path: &Path, roots: &[PathBuf], ignore_hidden: bool) -> bool {
    if !ignore_hidden {
        return false;
    }

    if let Some(relative_path) = roots.iter().find_map(|root| path.strip_prefix(root).ok()) {
        return relative_path
            .components()
            .any(|component| is_hidden_name(component.as_os_str().to_str(), ignore_hidden));
    }

    path.components()
        .any(|component| is_hidden_name(component.as_os_str().to_str(), ignore_hidden))
}

fn is_hidden_name(name: Option<&str>, ignore_hidden: bool) -> bool {
    ignore_hidden && name.is_some_and(|name| name.starts_with('.') && name != "." && name != "..")
}

fn codec_label(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("UNKNOWN")
        .to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};
    use std::{fs::File, io::Write};
    use tempfile::TempDir;

    #[test]
    fn detects_supported_audio_extensions_case_insensitively() {
        assert!(is_supported_audio_file("song.MP3"));
        assert!(is_supported_audio_file("song.flac"));
        assert!(is_supported_audio_file("song.wave"));
        assert!(is_supported_audio_file("song.OPUS"));
        assert!(!is_supported_audio_file("cover.jpg"));
        assert!(!is_supported_audio_file("README"));
    }

    #[test]
    fn parses_track_number_from_common_tag_values() {
        assert_eq!(parse_track_number("01"), Some(1));
        assert_eq!(parse_track_number("01/13"), Some(1));
        assert_eq!(parse_track_number("Track 07"), Some(7));
        assert_eq!(parse_track_number(""), None);
    }

    #[test]
    fn indexes_valid_wav_with_filename_fallbacks() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("No Tags.wav");
        write_silent_wav(&path).unwrap();

        let track = index_audio_file(&path).unwrap();

        assert_eq!(track.path, path);
        assert_eq!(track.title, "No Tags");
        assert_eq!(track.artist, "Unknown Artist");
        assert_eq!(track.album, "Unknown Album");
        assert_eq!(track.genre, None);
        assert_eq!(track.codec, "WAV");
        assert_eq!(track.sample_rate, Some(44_100));
        assert_eq!(track.channels, Some(1));
        assert_eq!(track.artwork, None);
        assert!(track.file_size > 44);
    }

    #[test]
    fn recursive_scan_indexes_audio_and_ignores_hidden_paths() {
        let temp = TempDir::new().unwrap();
        let visible = temp.path().join("album");
        let hidden = temp.path().join(".hidden");
        fs::create_dir_all(&visible).unwrap();
        fs::create_dir_all(&hidden).unwrap();
        let artwork_path = visible.join("cover.jpg");
        fs::write(&artwork_path, b"folder art placeholder").unwrap();
        write_silent_wav(&visible.join("track.wav")).unwrap();
        write_silent_wav(&hidden.join("secret.wav")).unwrap();
        fs::write(visible.join("notes.txt"), "not audio").unwrap();

        let report = LibraryIndexer::new([temp.path().to_path_buf()]).scan();

        assert_eq!(report.errors, Vec::new());
        assert_eq!(report.tracks.len(), 1);
        assert_eq!(report.tracks[0].title, "track");
        assert_eq!(report.tracks[0].artwork, Some(Artwork::File(artwork_path)));
    }

    #[test]
    fn scanner_records_corrupt_audio_without_failing_report() {
        let temp = TempDir::new().unwrap();
        let good = temp.path().join("good.wav");
        let bad = temp.path().join("bad.mp3");
        write_silent_wav(&good).unwrap();
        fs::write(&bad, b"not actually audio").unwrap();

        let report = LibraryIndexer::new([temp.path().to_path_buf()]).scan();

        assert_eq!(report.tracks.len(), 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, bad);
    }

    #[test]
    fn notify_events_are_debounced_into_index_and_remove_events() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("created.wav");
        write_silent_wav(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut pending = HashMap::new();

        record_notify_event(
            Event::new(EventKind::Create(CreateKind::File)).add_path(path.clone()),
            &[temp.path().to_path_buf()],
            true,
            &mut pending,
        );
        flush_pending(&tx, None, &mut pending);

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            LibraryEvent::TracksIndexed(tracks) if tracks.len() == 1 && tracks[0].path == path
        ));

        fs::remove_file(&path).unwrap();
        record_notify_event(
            Event::new(EventKind::Remove(RemoveKind::File)).add_path(path.clone()),
            &[temp.path().to_path_buf()],
            true,
            &mut pending,
        );
        flush_pending(&tx, None, &mut pending);

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            LibraryEvent::TrackRemoved(removed) if removed == path
        ));
    }

    #[test]
    fn remove_events_keep_directory_paths_for_catalog_reconciliation() {
        let temp = TempDir::new().unwrap();
        let album = temp.path().join("album");
        let (tx, _rx) = mpsc::channel();
        let mut pending = HashMap::new();

        record_notify_event(
            Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(album.clone()),
            &[temp.path().to_path_buf()],
            true,
            &mut pending,
        );

        assert_eq!(pending.get(&album), Some(&PendingPath::Remove));
        flush_pending(&tx, None, &mut pending);

        assert!(pending.is_empty());
    }

    #[test]
    fn background_watcher_emits_initial_scan_events() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("initial.wav");
        write_silent_wav(&path).unwrap();
        let (tx, rx) = mpsc::channel();

        let watcher = LibraryIndexer::with_options(
            [temp.path().to_path_buf()],
            IndexerOptions {
                debounce: Duration::from_millis(50),
                batch_size: 1,
                ..IndexerOptions::default()
            },
        )
        .start_watching(tx)
        .unwrap();

        let mut saw_scan_start = false;
        let mut saw_track = false;
        let mut saw_scan_finish = false;

        for _ in 0..5 {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                LibraryEvent::ScanStarted => saw_scan_start = true,
                LibraryEvent::TracksIndexed(tracks) => {
                    saw_track |= tracks.iter().any(|track| track.path == path);
                }
                LibraryEvent::ScanFinished => {
                    saw_scan_finish = true;
                    break;
                }
                LibraryEvent::ScanError(error) => panic!("unexpected scan error: {error:?}"),
                LibraryEvent::ScanProgress(_) => {}
                LibraryEvent::TracksRemoved(paths) => {
                    panic!("unexpected remove events: {paths:?}")
                }
                LibraryEvent::TrackRemoved(path) => panic!("unexpected remove event: {path:?}"),
            }
        }

        watcher.stop();

        assert!(saw_scan_start);
        assert!(saw_track);
        assert!(saw_scan_finish);
    }

    fn write_silent_wav(path: &Path) -> std::io::Result<()> {
        let sample_rate = 44_100u32;
        let channels = 1u16;
        let bits_per_sample = 16u16;
        let samples = 1_024u32;
        let data_len = samples * channels as u32 * (bits_per_sample as u32 / 8);
        let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = channels * (bits_per_sample / 8);

        let mut file = File::create(path)?;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data_len).to_le_bytes())?;
        file.write_all(b"WAVE")?;
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_len.to_le_bytes())?;
        file.write_all(&vec![0; data_len as usize])?;
        Ok(())
    }
}
