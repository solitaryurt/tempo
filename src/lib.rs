pub mod audio_analyzer;
pub mod catalog;
pub mod equalizer;
pub mod hotkeys;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod hypr;
pub mod library;
pub mod metadata_worker;
pub mod mpris;
pub mod perf;
pub mod playback;
pub mod snapshot;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod tray;
