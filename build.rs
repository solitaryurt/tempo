//! Build script that injects version metadata for the running binary.
//!
//! The self-update flow needs to know what version is currently
//! installed so it can compare against the latest GitHub release.
//! We get that from CI: when the release workflow builds a tagged
//! commit, GitHub Actions sets `GITHUB_REF_NAME` to the tag name
//! (e.g. `v0.1.0`). We forward that into `TEMPO_BUILD_VERSION` so
//! the binary can read it via `env!("TEMPO_BUILD_VERSION")`.
//!
//! Local `cargo build` invocations have no tag, so we fall back to
//! `<cargo-version>-dev` which makes the running binary easy to
//! distinguish from a real release in logs / settings UI without
//! tripping the "update available" comparison (since "0.1.0-dev"
//! does not parse as a release tag).
//!
//! Re-runs:
//! - `rerun-if-env-changed` lines below ensure the build script is
//!   re-evaluated when CI flips between an untagged push and a tag
//!   push within the same cache, otherwise the cached `OUT_DIR`
//!   would carry a stale version stamp.

use std::env;

fn main() {
    // Cargo's own version is the safe local default. CI overrides
    // this when building a release tag.
    let cargo_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());

    // GitHub Actions populates GITHUB_REF_NAME with the short ref
    // name. For tag pushes that's the tag itself ("v0.1.0"); for
    // branch pushes it's the branch name. We also accept an explicit
    // `TEMPO_BUILD_VERSION` override so a future packager (Flatpak,
    // distro maintainer) can stamp their own version without going
    // through GitHub Actions.
    let explicit = env::var("TEMPO_BUILD_VERSION").ok();
    let github_ref = env::var("GITHUB_REF_NAME").ok();

    let resolved = match (explicit, github_ref) {
        (Some(value), _) if !value.trim().is_empty() => value,
        // Only treat GITHUB_REF_NAME as a release version when it
        // looks like a `v<num>...` tag. Branch names like `master`
        // or PR refs like `123/merge` should not pollute the version.
        (_, Some(ref_name)) if looks_like_release_tag(&ref_name) => ref_name,
        _ => format!("{cargo_version}-dev"),
    };

    println!("cargo:rustc-env=TEMPO_BUILD_VERSION={resolved}");
    println!("cargo:rerun-if-env-changed=TEMPO_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
}

fn looks_like_release_tag(value: &str) -> bool {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix('v') else {
        return false;
    };
    // First character after the `v` must be a digit so we don't
    // pick up branch names that happen to start with `v` (e.g.
    // `verify-foo`).
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
}
