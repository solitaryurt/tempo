//! Self-update wiring on the [`TempoApp`] entity.
//!
//! Concerns split out of `mod.rs` to keep `TempoApp::new` short:
//!
//! - [`UpdateState`] holds runtime-only update bookkeeping (last poll,
//!   phase, error toast) so the UI can render without re-querying the
//!   network.
//! - [`UpdatePhase`] is the small state machine driving the pill in
//!   the top bar: Idle → Checking → UpToDate / Available / Downloading
//!   → Ready → Failed.
//! - [`TempoApp::start_update_poll`] kicks off the periodic GitHub
//!   poll and a manual-check entry point.
//! - [`TempoApp::start_update_install`] performs the staged download
//!   and either renames in place + restarts via `exec`, or hands off
//!   to the `tempo-updater` helper when an inline replace isn't
//!   safe.
//! - [`TempoApp::render_update_pill`] is the small SVG button that
//!   sits next to the metadata-sync pill in the top bar.
//!
//! All network and disk work happens on the background executor; the
//! UI thread only mutates `update_state` via short `this.update(cx,
//! …)` blocks.

use std::{path::PathBuf, sync::Arc, time::Instant};

use gpui::{
    AnyElement, Context, Image, ImageFormat, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, Styled, div, img, prelude::*, px, rgb,
};

use tempo::{
    perf,
    updates::{self, AUTO_POLL_INTERVAL, INITIAL_POLL_DELAY, ReleaseInfo, UpdateError},
};

use super::{TempoApp, theme::ThemeColors};

/// Runtime-only update state. Intentionally not persisted: poll
/// results are cheap to refresh on the next launch and persisting a
/// stale "update available" flag would be misleading after the user
/// updated out-of-band (apt, curl, distro package, etc.).
#[derive(Debug, Default, Clone)]
pub(crate) struct UpdateState {
    pub phase: UpdatePhase,
    /// Most recent successful poll, used so the manual "check now"
    /// flow can throttle obvious double-clicks.
    pub last_poll: Option<Instant>,
    /// Most recent release we *did* see a payload for, regardless of
    /// whether it was newer. Surfaced in the tooltip so the user can
    /// always read the latest known tag.
    pub latest_release: Option<ReleaseInfo>,
    /// Last error string from a poll or install attempt. Cleared on
    /// the next successful poll. Surfaced in the tooltip.
    pub last_error: Option<String>,
    /// Path of the staged binary that's ready to install. Set after
    /// download succeeds; consumed by the install path.
    pub staged_binary: Option<PathBuf>,
    /// Companion path for the staged updater helper. May be `None` on
    /// older releases that didn't ship it; in that case the install
    /// path falls back to [`updates::locate_updater`].
    pub staged_updater: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum UpdatePhase {
    /// No poll has run this session. Pill is hidden.
    #[default]
    Idle,
    /// Poll in flight. Pill shows a subtle spinner-ish glyph; click
    /// is a no-op while we're checking.
    Checking,
    /// Successfully polled and we are on the latest version. Pill
    /// hides itself but `UpdateState::latest_release` is set so the
    /// settings page can show "you're up to date" copy.
    UpToDate,
    /// Polled and a newer tag was found. Pill is visible; clicking
    /// transitions us to `Downloading`.
    Available,
    /// Asset download in flight. Pill shows the same icon as
    /// `Available` but at slightly reduced opacity; clicking is a
    /// no-op so we don't issue a second download.
    Downloading,
    /// Download complete and verified; user click installs.
    Ready,
    /// Latest poll or install attempt failed. Pill shows a warning
    /// glyph; clicking retries the poll.
    Failed,
}

impl TempoApp {
    /// Spawn the background poll loop. Runs once after
    /// [`INITIAL_POLL_DELAY`] then every [`AUTO_POLL_INTERVAL`]. The
    /// loop short-circuits for dev builds and unsupported platforms
    /// so neither incurs network cost.
    pub(super) fn start_update_poll(&self, cx: &mut Context<Self>) {
        if updates::is_dev_build() {
            perf::event("updates.skip", "dev_build");
            return;
        }
        if updates::asset_name_for_current_platform().is_none() {
            perf::event("updates.skip", "no_asset_for_platform");
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INITIAL_POLL_DELAY).await;
            loop {
                // Bail out cleanly when the entity is gone.
                if this
                    .update(cx, |app, cx| {
                        app.update_state.phase = UpdatePhase::Checking;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }

                let result = cx
                    .background_executor()
                    .spawn(async { updates::fetch_latest_release() })
                    .await;

                if this
                    .update(cx, |app, cx| {
                        app.apply_poll_result(result);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }

                cx.background_executor().timer(AUTO_POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Manual "Check now" entry point. Kicks off a one-shot poll if
    /// there isn't already one in flight. Click handler on the pill.
    pub(super) fn check_for_updates_now(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.update_state.phase,
            UpdatePhase::Checking | UpdatePhase::Downloading
        ) {
            return;
        }
        self.update_state.phase = UpdatePhase::Checking;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { updates::fetch_latest_release() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.apply_poll_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Begin the install flow: download the staged release on a
    /// background thread, then either replace the binary inline and
    /// restart via `exec`, or hand off to the helper. Idempotent –
    /// second click while download is in flight is ignored.
    pub(super) fn start_update_install(&mut self, cx: &mut Context<Self>) {
        let release = match (&self.update_state.phase, &self.update_state.latest_release) {
            (UpdatePhase::Available, Some(release)) => release.clone(),
            (UpdatePhase::Ready, _) => {
                self.finalize_install(cx);
                return;
            }
            (UpdatePhase::Failed, _) => {
                // Retry the poll first; user gets a second click to
                // actually install.
                self.check_for_updates_now(cx);
                return;
            }
            _ => return,
        };

        self.update_state.phase = UpdatePhase::Downloading;
        self.update_state.last_error = None;
        cx.notify();

        let dest = match updates::default_download_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.update_state.phase = UpdatePhase::Failed;
                self.update_state.last_error = Some(err.to_string());
                cx.notify();
                return;
            }
        };

        cx.spawn(async move |this, cx| {
            let release_for_thread = release.clone();
            let download = cx
                .background_executor()
                .spawn(async move { updates::download_release(&release_for_thread, &dest) })
                .await;
            let _ = this.update(cx, |app, cx| match download {
                Ok(downloaded) => {
                    app.update_state.staged_binary = Some(downloaded.binary_path);
                    app.update_state.staged_updater = downloaded.updater_path;
                    app.update_state.phase = UpdatePhase::Ready;
                    perf::event("updates.download.complete", format!("tag={}", release.tag));
                    cx.notify();
                    // Auto-progress to install once the download
                    // finishes — clicking the pill the first time
                    // is consent for the whole operation.
                    app.finalize_install(cx);
                }
                Err(err) => {
                    perf::event("updates.download.error", err.to_string());
                    app.update_state.phase = UpdatePhase::Failed;
                    app.update_state.last_error = Some(err.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Replace the running binary with the staged download and
    /// restart Tempo. Called automatically when [`start_update_install`]
    /// finishes the download successfully, but exposed separately so a
    /// future "install on quit" flow can reuse it.
    fn finalize_install(&mut self, cx: &mut Context<Self>) {
        let Some(downloaded) = self.update_state.staged_binary.clone() else {
            self.update_state.phase = UpdatePhase::Failed;
            self.update_state.last_error =
                Some("no staged binary; download did not complete".into());
            cx.notify();
            return;
        };

        let target = match std::env::current_exe() {
            Ok(path) => path,
            Err(err) => {
                self.update_state.phase = UpdatePhase::Failed;
                self.update_state.last_error = Some(format!("cannot resolve current_exe: {err}"));
                cx.notify();
                return;
            }
        };

        // Try the cheap, race-free path first: rename the new
        // binary on top of the running one. Linux allows this even
        // for the currently-running executable.
        match updates::try_inline_replace(&downloaded, &target) {
            Ok(()) => {
                perf::event("updates.install.inline_ok", target.display().to_string());
                self.relaunch_self(&target, cx);
                return;
            }
            Err(UpdateError::Unsupported(msg)) => {
                perf::event("updates.install.inline_unsupported", msg);
            }
            Err(err) => {
                perf::event("updates.install.inline_failed", err.to_string());
            }
        }

        // Helper-based fallback: spawn `tempo-updater` to wait for
        // us, swap the file, and re-launch.
        let helper = updates::locate_updater(self.update_state.staged_updater.as_deref());
        let Some(helper) = helper else {
            self.update_state.phase = UpdatePhase::Failed;
            self.update_state.last_error =
                Some("missing tempo-updater helper; reinstall Tempo manually".into());
            cx.notify();
            return;
        };

        if let Err(err) = updates::spawn_updater(&helper, &downloaded, &target) {
            perf::event("updates.install.helper_failed", err.to_string());
            self.update_state.phase = UpdatePhase::Failed;
            self.update_state.last_error = Some(err.to_string());
            cx.notify();
            return;
        }

        // Helper is running; quit so it can swap the file. The
        // helper itself re-launches us.
        perf::event(
            "updates.install.helper_spawned",
            target.display().to_string(),
        );
        cx.quit();
    }

    /// Restart the running app in place by execing the new binary.
    /// Only used after an inline-replace succeeded — at that point
    /// the on-disk file is already the new version, we just need to
    /// hand control to it.
    #[cfg(unix)]
    fn relaunch_self(&mut self, target: &std::path::Path, cx: &mut Context<Self>) {
        use std::os::unix::process::CommandExt as _;
        // Save state before we exec so the next process picks up
        // the most recent settings/queue/etc.
        self.save_app_state_now();
        let mut command = std::process::Command::new(target);
        command.args(std::env::args().skip(1));
        let err = command.exec();
        // exec() only returns on failure.
        perf::event("updates.relaunch.exec_failed", err.to_string());
        self.update_state.phase = UpdatePhase::Failed;
        self.update_state.last_error = Some(format!("relaunch failed: {err}"));
        cx.notify();
    }

    #[cfg(not(unix))]
    fn relaunch_self(&mut self, target: &std::path::Path, cx: &mut Context<Self>) {
        // On non-unix we can't exec; spawn detached and quit.
        let _ = std::process::Command::new(target)
            .args(std::env::args().skip(1))
            .spawn();
        cx.quit();
    }

    fn apply_poll_result(&mut self, result: Result<ReleaseInfo, UpdateError>) {
        match result {
            Ok(info) => {
                let current = updates::current_version();
                let newer = updates::is_release_newer(current, &info.tag);
                self.update_state.last_poll = Some(Instant::now());
                self.update_state.last_error = None;
                self.update_state.latest_release = Some(info.clone());
                self.update_state.phase = if newer {
                    UpdatePhase::Available
                } else {
                    UpdatePhase::UpToDate
                };
                perf::event(
                    "updates.poll.applied",
                    format!("current={} latest={} newer={}", current, info.tag, newer),
                );
            }
            Err(err) => {
                perf::event("updates.poll.error", err.to_string());
                self.update_state.last_error = Some(err.to_string());
                self.update_state.phase = UpdatePhase::Failed;
            }
        }
    }

    /// Render the small "update available" pill that sits next to
    /// the metadata-sync indicator in the top bar. Returns `None`
    /// for phases that should not be visible (Idle, UpToDate,
    /// Checking with no prior result).
    pub(super) fn render_update_pill(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        // Hide entirely until we have something to surface. Showing
        // a "checking" indicator on every launch would be noise.
        match self.update_state.phase {
            UpdatePhase::Idle | UpdatePhase::UpToDate => return None,
            UpdatePhase::Checking if self.update_state.latest_release.is_none() => {
                return None;
            }
            _ => {}
        }

        let colors = *self.colors();
        let phase = self.update_state.phase.clone();
        let glyph = self.update_pill_glyph(colors, &phase);
        let label = self.update_pill_tooltip();
        let id_str: SharedString = format!("update-pill-{:?}", phase).into();

        let active = matches!(
            phase,
            UpdatePhase::Available | UpdatePhase::Ready | UpdatePhase::Failed
        );
        let bg = if active {
            colors.button_hover
        } else {
            colors.elevated
        };
        let border = if matches!(phase, UpdatePhase::Failed) {
            colors.border_strong
        } else if active {
            colors.accent
        } else {
            colors.border
        };

        let pill = div()
            .id(id_str)
            .h(px(26.0))
            .px_2()
            .rounded_full()
            .bg(rgb(bg))
            .border_1()
            .border_color(rgb(border))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(glyph)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_strong))
                    .whitespace_nowrap()
                    .child(self.update_pill_label()),
            )
            .hover(move |this| this.bg(rgb(colors.button_hover)))
            .active(|this| this.opacity(0.82))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                    this.handle_update_pill_click(cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            );

        Some(self.with_tooltip(pill, "update-pill-tooltip", label, cx))
    }

    fn handle_update_pill_click(&mut self, cx: &mut Context<Self>) {
        match self.update_state.phase {
            UpdatePhase::Available | UpdatePhase::Ready => self.start_update_install(cx),
            UpdatePhase::Failed => self.check_for_updates_now(cx),
            UpdatePhase::Checking | UpdatePhase::Downloading => {
                // No-op while work is in flight.
            }
            UpdatePhase::Idle | UpdatePhase::UpToDate => {
                // Defensive: shouldn't render a clickable pill in
                // these states, but if a future caller does we treat
                // it as "check now".
                self.check_for_updates_now(cx);
            }
        }
    }

    fn update_pill_label(&self) -> SharedString {
        match self.update_state.phase {
            UpdatePhase::Available => "Update".into(),
            UpdatePhase::Downloading => "Downloading…".into(),
            UpdatePhase::Ready => "Restart".into(),
            UpdatePhase::Failed => "Update failed".into(),
            UpdatePhase::Checking => "Checking…".into(),
            UpdatePhase::Idle | UpdatePhase::UpToDate => SharedString::default(),
        }
    }

    fn update_pill_tooltip(&self) -> &'static str {
        match self.update_state.phase {
            UpdatePhase::Available => "A newer Tempo release is available. Click to install.",
            UpdatePhase::Downloading => "Downloading the new release in the background.",
            UpdatePhase::Ready => "Update downloaded. Click to restart Tempo.",
            UpdatePhase::Failed => {
                // Surface the inner error in the tooltip so it's at
                // least visible to power users who hover; we can't
                // return a borrowed dynamic string here cheaply, so
                // for now the pill carries a static label and the
                // perf log carries the detail.
                "Update failed. Click to retry."
            }
            UpdatePhase::Checking => "Checking for updates…",
            UpdatePhase::Idle | UpdatePhase::UpToDate => "Tempo is up to date.",
        }
    }

    fn update_pill_glyph(&self, colors: ThemeColors, phase: &UpdatePhase) -> AnyElement {
        let stroke = match phase {
            UpdatePhase::Available | UpdatePhase::Ready => colors.accent,
            UpdatePhase::Failed => colors.border_strong,
            _ => colors.text_muted,
        };
        let stroke_hex = format!("#{:06x}", stroke);

        // 2D "download arrow into tray" glyph — a downward arrow
        // above a horizontal line. Same visual idiom Chrome / VS
        // Code use for "update available". We reuse it for `Ready`
        // because the meaning ("install pending") is the same.
        let arrow_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
<path d="M12 4v12" stroke="{stroke_hex}" stroke-width="2" stroke-linecap="round"/>
<path d="M6 11l6 6 6-6" fill="none" stroke="{stroke_hex}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
<path d="M5 20h14" stroke="{stroke_hex}" stroke-width="2" stroke-linecap="round"/>
</svg>"#
        );

        // Failed phase swaps to a warning triangle to make the
        // problem unambiguous at a glance.
        let warn_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
<path d="M12 3l10 18H2L12 3z" fill="none" stroke="{stroke_hex}" stroke-width="2" stroke-linejoin="round"/>
<path d="M12 10v5" stroke="{stroke_hex}" stroke-width="2" stroke-linecap="round"/>
<circle cx="12" cy="18" r="1" fill="{stroke_hex}"/>
</svg>"#
        );

        let svg = match phase {
            UpdatePhase::Failed => warn_svg,
            _ => arrow_svg,
        };

        img(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.into_bytes(),
        )))
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .into_any_element()
    }
}
