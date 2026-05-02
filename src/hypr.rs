//! Hyprland-specific window-hide / show helpers.
//!
//! Hyprland is a tiling Wayland compositor that, like most tilers,
//! does not implement `xdg_toplevel.set_minimized()` in any visible
//! way for tiled windows — the request is silently no-op'd. That
//! means our standard `Window::minimize_window()` call (which is
//! all `xdg_toplevel.set_minimized()` under the hood on Wayland)
//! does nothing on Hyprland. The window stays exactly where it
//! was.
//!
//! The Hyprland-idiomatic substitute is to move the window to a
//! special workspace. Special workspaces are like normal workspaces
//! except they're not numbered and only render when explicitly
//! shown via `togglespecialworkspace`. Moving a window to one
//! makes it disappear from every regular workspace — exactly the
//! "hide" behaviour the user expects from clicking the tray.
//!
//! `hyprctl dispatch movetoworkspacesilent` is the right primitive:
//!
//! - `silent` keeps the operation from bringing the destination
//!   workspace into view (which is what we want — we're hiding,
//!   not switching).
//! - The selector `class:^(tempo)$` keys off the window's `app_id`
//!   we set in `main.rs`. No need to look up the window by
//!   address.
//!
//! This module is gated on `cfg(all(unix, not(target_os = "macos")))`
//! at the import site (`src/lib.rs`) and is itself a no-op on any
//! environment that isn't Hyprland — `is_running_under_hyprland`
//! checks the `HYPRLAND_INSTANCE_SIGNATURE` env var that Hyprland
//! sets on every spawned process.

use std::process::Command;

use crate::perf;

/// Special workspace name we park the Tempo window in when
/// "hiding". Picked to be unique across the user's other tools so
/// we don't collide with whatever they happen to use for personal
/// scratchpads (e.g. `special:scratchpad`, `special:terminal`).
const HIDDEN_SPECIAL_WORKSPACE: &str = "tempo-tray";

/// CSS-style selector targeting the Tempo window by its
/// `app_id`. Set in `main.rs` via `WindowOptions::app_id`. The
/// regex anchors are mandatory — Hyprland matches with regex and
/// "tempo" without anchors would also match e.g. "tempo-popover"
/// if we ever add one.
const WINDOW_SELECTOR: &str = "class:^(tempo)$";

/// `true` if the current process was spawned by Hyprland. Cheap
/// check that doesn't shell out — Hyprland sets this env var on
/// every child it launches.
pub fn is_running_under_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Hide the Tempo window by moving it to a dedicated special
/// workspace. No-op on non-Hyprland environments — the caller is
/// expected to also call the standard `Window::minimize_window()`
/// path; on Hyprland the standard path silently fails so this is
/// what actually achieves the hide; on KDE/GNOME the standard
/// path works and this is unreachable.
///
/// Returns `true` if the dispatch was issued (regardless of
/// whether `hyprctl` ultimately succeeded), `false` if we skipped
/// because we're not on Hyprland.
pub fn hide_window() -> bool {
    if !is_running_under_hyprland() {
        return false;
    }
    let target =
        format!("movetoworkspacesilent special:{HIDDEN_SPECIAL_WORKSPACE},{WINDOW_SELECTOR}");
    run_hyprctl_dispatch(&target, "hypr.hide_window");
    true
}

/// Show the Tempo window by yanking it out of the hidden special
/// workspace and onto whatever workspace currently has focus, then
/// focusing it. Mirror of `hide_window`.
///
/// We use two dispatches: first `movetoworkspacesilent` to land
/// the window on the active workspace, then `focuswindow` to make
/// sure it's the active client and not just sitting behind the
/// previously-focused tile.
pub fn show_window() -> bool {
    if !is_running_under_hyprland() {
        return false;
    }
    // `name:` selects a workspace by its name; `~` is Hyprland's
    // shorthand for "the last-used regular (non-special)
    // workspace". `e+0` would be "current" but breaks if the
    // current workspace is the hidden special one (unlikely but
    // possible — the user could have toggled it via
    // togglespecialworkspace). Using `~` sidesteps that.
    let move_arg = format!("movetoworkspacesilent ~,{WINDOW_SELECTOR}");
    run_hyprctl_dispatch(&move_arg, "hypr.show_window.move");
    let focus_arg = format!("focuswindow {WINDOW_SELECTOR}");
    run_hyprctl_dispatch(&focus_arg, "hypr.show_window.focus");
    true
}

fn run_hyprctl_dispatch(args: &str, perf_tag: &'static str) {
    // `hyprctl dispatch <subcmd> <args>` is a single string after
    // the `dispatch` token; we split on whitespace and pass each
    // piece as its own argv. Hyprland accepts both forms.
    let pieces: Vec<&str> = args.split_whitespace().collect();
    let mut command = Command::new("hyprctl");
    command.arg("dispatch");
    for piece in &pieces {
        command.arg(piece);
    }
    match command.output() {
        Ok(output) => {
            if !output.status.success() {
                perf::event(
                    perf_tag,
                    format!(
                        "exit={} stderr={}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                );
            }
        }
        Err(error) => {
            perf::event(perf_tag, format!("spawn_err={error}"));
        }
    }
}
