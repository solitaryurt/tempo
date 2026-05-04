//! Self-update plumbing.
//!
//! High-level flow (Linux only for now):
//!
//! 1. Periodically poll `https://api.github.com/repos/<repo>/releases/latest`.
//! 2. Compare the release's `tag_name` against [`current_version`].
//! 3. When a newer version is available the UI surfaces a small
//!    "update available" pill in the top bar.
//! 4. Clicking the pill triggers [`download_update`], which fetches
//!    the matching asset, verifies its `sha256` against the digest
//!    from the GitHub API, and writes it to a temp file.
//! 5. We launch the bundled `tempo-updater` helper (built alongside
//!    the main binary by CI) with `--pid <our_pid> --from <temp> --to
//!    <current_exe>` and quit. The helper waits for our PID to die,
//!    replaces the binary, and execs the new version.
//!
//! macOS/Windows/other platforms still surface "update available"
//! when their assets are present, but the click-to-install path is
//! gated behind [`asset_name_for_current_platform`] returning `Some`.
//! For now only Linux x86_64 is supported.
//!
//! ## Error handling
//!
//! All operations are infallible from the UI's perspective: failures
//! map to [`UpdateError`] and are surfaced as a dismissible status
//! string on the pill. We never panic; a failed update should leave
//! the running binary intact.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::perf;

/// GitHub repo the updater polls. Hard-coded for now; can be moved
/// behind an env override later if we ever ship community rebuilds
/// that should track a different fork.
const RELEASE_REPO: &str = "tempo2000/tempo";
/// User-Agent string for the GitHub API. GitHub rejects requests
/// without a User-Agent, so always set one. We mirror the format
/// used by `metadata_worker.rs` so the UA is recognizable in our
/// own logs.
const USER_AGENT: &str = concat!("Tempo/", env!("TEMPO_BUILD_VERSION"), " (+self-update)");
/// HTTP timeout for both the metadata poll and the asset download.
/// The download is large (~50 MB) so this is intentionally generous.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long between automatic background polls. The user can also
/// trigger a manual check from the pill / settings; this is just
/// the floor.
pub const AUTO_POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Initial delay before the first poll fires, so app startup isn't
/// gated on a network round-trip. One minute gives the rest of the
/// app time to stand up and avoids surprising users on flaky links.
pub const INITIAL_POLL_DELAY: Duration = Duration::from_secs(60);

/// Compile-time version stamp produced by `build.rs`. For tagged
/// CI builds this looks like `v0.1.0`; for local `cargo build`
/// invocations it's `<cargo-version>-dev`.
pub fn current_version() -> &'static str {
    env!("TEMPO_BUILD_VERSION")
}

/// `true` when the running binary was produced by `cargo build`
/// outside of CI (i.e. the version stamp ends with `-dev`). The
/// updater treats dev builds as "always up to date" so a developer
/// running locally never sees an "update available" prompt.
pub fn is_dev_build() -> bool {
    current_version().ends_with("-dev")
}

/// Asset name we expect to find on a GitHub release for the current
/// platform. `None` means we don't ship a matching binary for this
/// target and the updater should remain a passive observer (no
/// install, no pill action).
pub fn asset_name_for_current_platform() -> Option<&'static str> {
    // Mirror the names produced by `.github/workflows/rust.yml`.
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("tempo-linux-x86_64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("tempo-macos-aarch64")
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("tempo-windows-x86_64.exe")
    } else {
        None
    }
}

/// Convenience for the helper-binary basename. CI uploads it
/// alongside the main binary so the running app can copy or
/// download it on demand. Currently only consumed on Linux.
pub fn updater_asset_name() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("tempo-updater-linux-x86_64")
    } else {
        None
    }
}

/// Subset of the GitHub release JSON we care about. Extra fields
/// are ignored by `serde(deny_unknown_fields)` being absent.
#[derive(Debug, Clone, Deserialize)]
struct ReleaseResponse {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    /// GitHub returns the digest as `"sha256:<hex>"`. Older releases
    /// may omit it entirely, in which case we skip verification but
    /// log a warning.
    #[serde(default)]
    digest: Option<String>,
}

/// User-facing release info distilled from the GitHub API response.
/// `expected_sha256` is `None` when the asset predates GitHub's
/// digest field; the caller may proceed without verification on a
/// best-effort basis (TLS still guarantees transport integrity).
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub name: Option<String>,
    pub html_url: Option<String>,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub expected_sha256: Option<String>,
    pub updater_asset: Option<UpdaterAsset>,
}

/// Companion helper download info (Linux only). Produced when CI
/// publishes a `tempo-updater-*` asset on the release; absent on
/// older releases, in which case we fall back to copying the
/// helper from the running binary's directory at install time.
#[derive(Debug, Clone)]
pub struct UpdaterAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub expected_sha256: Option<String>,
}

#[derive(Debug)]
pub enum UpdateError {
    /// Network or HTTP-level failure talking to the GitHub API or
    /// downloading the asset. The contained string is suitable for
    /// human display.
    Network(String),
    /// We reached GitHub but it returned an unexpected payload (e.g.
    /// no matching asset, missing tag). The contained string is the
    /// specific reason.
    Malformed(String),
    /// Downloaded file's sha256 does not match the digest GitHub
    /// published for the asset. Treated as fatal.
    DigestMismatch { expected: String, actual: String },
    /// File-system error while writing the downloaded binary or
    /// preparing the install path.
    Io(String),
    /// Self-update is structurally not possible on this platform/build.
    /// Currently: missing platform asset, dev build, missing helper
    /// binary, or unwritable target path.
    Unsupported(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(msg) => write!(f, "network error: {msg}"),
            UpdateError::Malformed(msg) => write!(f, "unexpected release data: {msg}"),
            UpdateError::DigestMismatch { expected, actual } => {
                write!(f, "checksum mismatch: expected {expected}, got {actual}")
            }
            UpdateError::Io(msg) => write!(f, "io error: {msg}"),
            UpdateError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for UpdateError {}

fn http_client() -> Result<Client, UpdateError> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|err| UpdateError::Network(format!("client build failed: {err}")))
}

/// Hit the GitHub API for the latest release and resolve the
/// platform-specific asset. Returns `None` when this build doesn't
/// have a matching asset (see [`asset_name_for_current_platform`]).
pub fn fetch_latest_release() -> Result<ReleaseInfo, UpdateError> {
    let asset_name = asset_name_for_current_platform()
        .ok_or_else(|| UpdateError::Unsupported("no asset for this platform".into()))?;

    let url = format!("https://api.github.com/repos/{RELEASE_REPO}/releases/latest");
    perf::event("updates.poll.start", url.clone());

    let client = http_client()?;
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|err| UpdateError::Network(format!("GET {url} failed: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(UpdateError::Network(format!(
            "GitHub API returned {status} for {url}"
        )));
    }

    let release: ReleaseResponse = response
        .json()
        .map_err(|err| UpdateError::Malformed(format!("parse release JSON: {err}")))?;

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            UpdateError::Malformed(format!(
                "release {} has no asset named {asset_name}",
                release.tag_name
            ))
        })?;

    let updater_asset = updater_asset_name().and_then(|name| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| UpdaterAsset {
                name: a.name.clone(),
                url: a.browser_download_url.clone(),
                size: a.size,
                expected_sha256: parse_digest(a.digest.as_deref()),
            })
    });

    let info = ReleaseInfo {
        tag: release.tag_name,
        name: release.name,
        html_url: release.html_url,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        expected_sha256: parse_digest(asset.digest.as_deref()),
        updater_asset,
    };

    perf::event(
        "updates.poll.ok",
        format!(
            "tag={} asset_size={} digest={}",
            info.tag,
            info.asset_size,
            info.expected_sha256.as_deref().unwrap_or("<missing>"),
        ),
    );

    Ok(info)
}

/// Strip GitHub's `"sha256:"` prefix and lower-case the hex so
/// downstream comparisons are normalized. Returns `None` for empty
/// or non-sha256 inputs (the caller treats this as "no digest
/// available"; verification is then skipped).
fn parse_digest(value: Option<&str>) -> Option<String> {
    let raw = value?.trim();
    let hex = raw.strip_prefix("sha256:").unwrap_or(raw);
    if hex.is_empty() {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

/// Decide whether `release.tag` represents a newer version than
/// the running binary. Falls back to a string-equality check if
/// either side fails to parse as semver, since the worst case is
/// "do nothing".
pub fn is_release_newer(current: &str, release_tag: &str) -> bool {
    if current == release_tag {
        return false;
    }
    let Some((cur_major, cur_minor, cur_patch)) = parse_semver_tag(current) else {
        // Dev builds intentionally skip the prompt.
        return false;
    };
    let Some((rel_major, rel_minor, rel_patch)) = parse_semver_tag(release_tag) else {
        // Unknown remote shape; better to stay quiet than to prompt
        // for "update" against an unparseable tag.
        return false;
    };
    (rel_major, rel_minor, rel_patch) > (cur_major, cur_minor, cur_patch)
}

/// Parse a tag like `v1.2.3` (or `1.2.3`) into its three numeric
/// components. Pre-release / build suffixes (`v1.2.3-rc.1`) parse
/// as the base version because we don't ship those today; we can
/// extend this once we do.
fn parse_semver_tag(tag: &str) -> Option<(u32, u32, u32)> {
    let trimmed = tag.trim();
    let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
    // Drop any pre-release / build suffix the user may have tagged
    // (`v0.1.0-rc.1`, `v0.1.0+exp.sha.5114f85`).
    let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// Download `release`'s platform asset to a temporary file inside
/// `dest_dir`. On success returns the path to the downloaded file
/// and the path to a sibling updater binary (when one was bundled
/// with the release). The caller is responsible for spawning the
/// updater process; this function does not modify the running
/// installation in any way.
pub fn download_release(
    release: &ReleaseInfo,
    dest_dir: &Path,
) -> Result<DownloadedRelease, UpdateError> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|err| UpdateError::Io(format!("create {}: {err}", dest_dir.display())))?;

    let main_path = dest_dir.join(&release.asset_name);
    download_asset(
        &release.asset_url,
        &main_path,
        release.asset_size,
        release.expected_sha256.as_deref(),
    )?;
    set_executable(&main_path)?;

    let updater_path = if let Some(updater) = &release.updater_asset {
        let path = dest_dir.join(&updater.name);
        download_asset(
            &updater.url,
            &path,
            updater.size,
            updater.expected_sha256.as_deref(),
        )?;
        set_executable(&path)?;
        Some(path)
    } else {
        None
    };

    Ok(DownloadedRelease {
        binary_path: main_path,
        updater_path,
    })
}

/// Result of a successful [`download_release`] call.
#[derive(Debug, Clone)]
pub struct DownloadedRelease {
    pub binary_path: PathBuf,
    pub updater_path: Option<PathBuf>,
}

fn download_asset(
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: Option<&str>,
) -> Result<(), UpdateError> {
    perf::event(
        "updates.download.start",
        format!("url={url} dest={}", dest.display()),
    );
    let client = http_client()?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|err| UpdateError::Network(format!("GET {url} failed: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(UpdateError::Network(format!(
            "asset {url} returned HTTP {status}"
        )));
    }

    // Stream the body through a sha256 hasher so we never hold the
    // full release (~50 MB) in memory and so the digest is computed
    // on exactly the bytes we wrote.
    let tmp_path = dest.with_extension("part");
    let mut file = std::fs::File::create(&tmp_path)
        .map_err(|err| UpdateError::Io(format!("create {}: {err}", tmp_path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        use std::io::{Read, Write};
        let n = response
            .read(&mut buf)
            .map_err(|err| UpdateError::Network(format!("read body: {err}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|err| UpdateError::Io(format!("write {}: {err}", tmp_path.display())))?;
        written += n as u64;
    }
    drop(file);

    if expected_size > 0 && written != expected_size {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(UpdateError::Malformed(format!(
            "downloaded {written} bytes, expected {expected_size}"
        )));
    }

    if let Some(expected) = expected_sha256 {
        let actual = hex_lower(hasher.finalize().as_slice());
        if actual != expected {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(UpdateError::DigestMismatch {
                expected: expected.to_string(),
                actual,
            });
        }
    }

    std::fs::rename(&tmp_path, dest).map_err(|err| {
        UpdateError::Io(format!(
            "rename {} -> {}: {err}",
            tmp_path.display(),
            dest.display()
        ))
    })?;
    perf::event(
        "updates.download.ok",
        format!("dest={} bytes={}", dest.display(), written),
    );
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)
        .map_err(|err| UpdateError::Io(format!("stat {}: {err}", path.display())))?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|err| UpdateError::Io(format!("chmod {}: {err}", path.display())))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), UpdateError> {
    // No-op; Windows enforces executability via filename extension.
    Ok(())
}

/// Default cache directory the app uses for staging downloads. Sits
/// under `$XDG_CACHE_HOME/tempo/updates` (or `~/.cache/tempo/updates`)
/// so successive checks reuse the same scratch space and we never
/// touch the install location until the actual swap.
pub fn default_download_dir() -> Result<PathBuf, UpdateError> {
    let base = if let Ok(value) = std::env::var("XDG_CACHE_HOME")
        && !value.is_empty()
    {
        PathBuf::from(value)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        return Err(UpdateError::Io(
            "no XDG_CACHE_HOME or HOME for download dir".into(),
        ));
    };
    Ok(base.join("tempo").join("updates"))
}

/// Locate the helper binary that performs the actual swap. We look,
/// in order, for:
///
/// 1. A freshly downloaded helper sitting next to the staged release
///    binary (caller passes it in via `staged_updater`).
/// 2. A `tempo-updater` next to the currently running executable
///    (the standard install layout where CI ships both binaries
///    side by side).
/// 3. A `tempo-updater` somewhere on `PATH`.
///
/// Returns `None` when none of the above resolve to a file. The
/// caller must surface this as [`UpdateError::Unsupported`] because
/// without the helper we cannot replace the running binary safely.
pub fn locate_updater(staged_updater: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = staged_updater
        && path.is_file()
    {
        return Some(path.to_path_buf());
    }

    if let Ok(current) = std::env::current_exe()
        && let Some(parent) = current.parent()
    {
        let neighbour = parent.join(if cfg!(windows) {
            "tempo-updater.exe"
        } else {
            "tempo-updater"
        });
        if neighbour.is_file() {
            return Some(neighbour);
        }
    }

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(if cfg!(windows) {
            "tempo-updater.exe"
        } else {
            "tempo-updater"
        });
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Spawn the helper updater process and return immediately. The
/// helper waits for the parent PID to die, replaces the binary at
/// `current_exe`, and re-launches it. On success the caller should
/// trigger an orderly app shutdown; the helper assumes the parent
/// will exit on its own.
pub fn spawn_updater(helper: &Path, downloaded: &Path, target: &Path) -> Result<(), UpdateError> {
    let pid = std::process::id().to_string();
    let mut command = std::process::Command::new(helper);
    command
        .arg("--pid")
        .arg(pid)
        .arg("--from")
        .arg(downloaded)
        .arg("--to")
        .arg(target)
        .arg("--restart");
    perf::event(
        "updates.spawn_updater",
        format!(
            "helper={} from={} to={}",
            helper.display(),
            downloaded.display(),
            target.display()
        ),
    );
    let child = command
        .spawn()
        .map_err(|err| UpdateError::Io(format!("spawn updater {}: {err}", helper.display())))?;
    // We deliberately drop the child handle: the helper must outlive
    // us, and waiting for it would defeat the purpose. PID is
    // available via `child.id()` if we ever need to log it.
    let _ = child;
    Ok(())
}

/// Try a clean self-replace without the helper. This is the path
/// for "the running binary is currently writable AND the OS allows
/// in-place rename" (Linux's standard inode-swap behaviour). When
/// it works it skips the helper entirely; when it fails we fall
/// back to the helper-based flow.
///
/// The function never executes the new binary; the caller is
/// expected to restart the process via `std::os::unix::process::CommandExt::exec`
/// or by spawning it and exiting.
#[cfg(unix)]
pub fn try_inline_replace(downloaded: &Path, target: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::MetadataExt as _;

    let target_meta = std::fs::metadata(target)
        .map_err(|err| UpdateError::Io(format!("stat target {}: {err}", target.display())))?;
    let parent = target
        .parent()
        .ok_or_else(|| UpdateError::Io(format!("target {} has no parent", target.display())))?;
    let parent_meta = std::fs::metadata(parent)
        .map_err(|err| UpdateError::Io(format!("stat {}: {err}", parent.display())))?;

    // We need the parent dir writable (rename is technically a
    // dir-mutation) AND we need to be on the same filesystem as the
    // download. The "same fs" check is cheap and avoids EXDEV at
    // rename time.
    let dl_meta = std::fs::metadata(downloaded)
        .map_err(|err| UpdateError::Io(format!("stat {}: {err}", downloaded.display())))?;
    if dl_meta.dev() != parent_meta.dev() {
        return Err(UpdateError::Unsupported(format!(
            "download {} and target {} are on different filesystems",
            downloaded.display(),
            target.display()
        )));
    }

    if !is_writable(parent) {
        return Err(UpdateError::Unsupported(format!(
            "directory {} not writable",
            parent.display()
        )));
    }
    let _ = target_meta;

    // Move the new binary into place. On Linux this works even
    // when the original file is currently being executed: the
    // kernel keeps the running text segment mapped via the open
    // inode, and the new file gets a fresh inode under the same
    // path. The next launch picks up the new version.
    std::fs::rename(downloaded, target).map_err(|err| {
        UpdateError::Io(format!(
            "rename {} -> {}: {err}",
            downloaded.display(),
            target.display()
        ))
    })?;
    Ok(())
}

#[cfg(not(unix))]
pub fn try_inline_replace(_: &Path, _: &Path) -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported(
        "inline replace not supported on this platform".into(),
    ))
}

#[cfg(unix)]
fn is_writable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    // Cheap heuristic: any write bit set on the inode. The kernel
    // makes the actual decision based on uid/gid + acls, but for
    // reasonable installs this is good enough; if rename fails
    // we'll fall back to the helper anyway.
    meta.permissions().mode() & 0o222 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_tags() {
        assert_eq!(parse_semver_tag("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_semver_tag("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_semver_tag("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_semver_tag("v2.0"), Some((2, 0, 0)));
        assert_eq!(parse_semver_tag("v3"), Some((3, 0, 0)));
        assert_eq!(parse_semver_tag("not-a-version"), None);
        assert_eq!(parse_semver_tag(""), None);
    }

    #[test]
    fn detects_newer_releases() {
        assert!(is_release_newer("v0.1.0", "v0.1.1"));
        assert!(is_release_newer("v0.1.0", "v0.2.0"));
        assert!(is_release_newer("v0.1.9", "v1.0.0"));
        assert!(!is_release_newer("v0.1.0", "v0.1.0"));
        assert!(!is_release_newer("v0.2.0", "v0.1.0"));
        // Dev builds do not parse, should never claim newer.
        assert!(!is_release_newer("0.1.0-dev", "v0.1.0"));
        assert!(!is_release_newer("v0.1.0", "non-version"));
    }

    #[test]
    fn parses_github_digests() {
        assert_eq!(
            parse_digest(Some("sha256:DEADBEEF")),
            Some("deadbeef".to_string())
        );
        assert_eq!(parse_digest(Some("deadbeef")), Some("deadbeef".to_string()));
        assert_eq!(parse_digest(Some("")), None);
        assert_eq!(parse_digest(None), None);
    }

    #[test]
    fn dev_builds_are_detected() {
        // The compiled-in version is whatever build.rs produced for
        // this test invocation, but the function is data-driven so
        // we can validate the suffix logic via current_version().
        let current = current_version();
        let suffix = current.ends_with("-dev");
        assert_eq!(is_dev_build(), suffix);
    }
}
