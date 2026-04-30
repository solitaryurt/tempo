//! Equalizer panel — vertical sliders, profile picker, save/load.
//!
//! Triggered from the EQ icon button to the left of the volume mute
//! control in the player bar. The button emits
//! [`super::player::PlayerEvent`] which the parent translates into
//! [`TempoApp::toggle_eq_panel`]. The panel itself renders as an
//! anchored overlay in the root view (alongside other transient
//! menus).
//!
//! ## Layout
//!
//! Header: profile picker dropdown, save/save-as actions, EQ on/off
//! switch, preamp slider.
//!
//! Body: 10 vertical sliders, one per ISO octave band, with a 0 dB
//! centerline, ±12 dB ticks, and per-slider numeric readout.
//!
//! Footer: "Reset to Flat" button.

use super::*;
use tempo::equalizer::{
    BAND_COUNT, BAND_GAIN_LIMIT_DB, BAND_LABELS, BUILTIN_PRESETS, EqProfile, EqProfileRef,
    PREAMP_LIMIT_DB, find_builtin_preset, new_profile_id,
};

/// Total height of a slider's interactive track. The thumb travels
/// over `[-BAND_GAIN_LIMIT_DB, +BAND_GAIN_LIMIT_DB]` mapped onto this
/// range; 0 dB is at the midpoint.
pub(super) const EQ_SLIDER_HEIGHT: f32 = 160.0;
pub(super) const EQ_SLIDER_TRACK_W: f32 = 6.0;
pub(super) const EQ_SLIDER_THUMB_W: f32 = 18.0;
pub(super) const EQ_SLIDER_THUMB_H: f32 = 12.0;
pub(super) const EQ_PANEL_W: f32 = 480.0;

impl TempoApp {
    /// Toggle the EQ panel open/closed. Mouse-down position is used
    /// to anchor the panel above the trigger button.
    pub(super) fn toggle_eq_panel(&mut self, anchor: Point<Pixels>) {
        if self.eq_panel_open {
            self.eq_panel_open = false;
            self.eq_profile_menu_open = false;
        } else {
            self.eq_panel_anchor = anchor;
            self.eq_panel_open = true;
        }
    }

    /// Close the panel without committing any in-flight save-as input.
    pub(super) fn close_eq_panel(&mut self) -> bool {
        if !self.eq_panel_open {
            return false;
        }
        self.eq_panel_open = false;
        self.eq_profile_menu_open = false;
        self.eq_slider_drag = None;
        self.eq_profile_save_as = None;
        self.eq_profile_save_as_focus_handle = None;
        self.eq_profile_delete_confirm = None;
        true
    }

    pub(super) fn set_eq_band_gain(&mut self, band: usize, gain_db: f32) {
        self.eq_state.set_band_gain_db(band, gain_db);
        self.save_app_state();
    }

    pub(super) fn set_eq_preamp(&mut self, gain_db: f32) {
        self.eq_state.set_preamp_db(gain_db);
        self.save_app_state();
    }

    pub(super) fn toggle_eq_bypass(&mut self) {
        let new_bypass = !self.eq_state.bypass();
        self.eq_state.set_bypass(new_bypass);
        self.save_app_state();
    }

    pub(super) fn reset_eq_to_flat(&mut self) {
        let flat = [0.0f32; BAND_COUNT];
        self.eq_state.load_profile(&flat, 0.0, false);
        self.eq_active_profile = Some(EqProfileRef::Builtin("Flat".to_string()));
        self.save_app_state();
    }

    pub(super) fn load_eq_profile(&mut self, profile_ref: &EqProfileRef) {
        let (gains, preamp) = match profile_ref {
            EqProfileRef::Builtin(name) => {
                let Some(preset) = find_builtin_preset(name) else {
                    return;
                };
                (preset.gains_db, preset.preamp_db)
            }
            EqProfileRef::User(id) => {
                let Some(profile) = self.eq_profiles.iter().find(|p| p.id == *id) else {
                    return;
                };
                (profile.gains_db, profile.preamp_db)
            }
        };
        // Loading a profile turns the EQ on. (Loading "Flat" is the
        // user's intent to be flat *and active*; if they wanted the
        // EQ disabled, they'd toggle bypass directly.)
        self.eq_state.load_profile(&gains, preamp, false);
        self.eq_active_profile = Some(profile_ref.clone());
        self.eq_profile_menu_open = false;
        self.save_app_state();
    }

    /// Save the current live values back into the active *user*
    /// profile. Built-in profiles are read-only — call sites guard.
    pub(super) fn save_eq_to_active_profile(&mut self) {
        let Some(EqProfileRef::User(id)) = self.eq_active_profile.clone() else {
            return;
        };
        let gains = self.eq_state.gains_db();
        let preamp = self.eq_state.preamp_db();
        if let Some(profile) = self.eq_profiles.iter_mut().find(|p| p.id == id) {
            profile.gains_db = gains;
            profile.preamp_db = preamp;
        }
        self.save_app_state();
    }

    pub(super) fn create_eq_profile_from_current(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let profile = EqProfile {
            id: new_profile_id(),
            name,
            preamp_db: self.eq_state.preamp_db(),
            gains_db: self.eq_state.gains_db(),
        };
        let id = profile.id.clone();
        self.eq_profiles.push(profile);
        self.eq_active_profile = Some(EqProfileRef::User(id));
        self.save_app_state();
    }

    pub(super) fn delete_eq_profile(&mut self, id: &str) {
        let prev_active = self.eq_active_profile.clone();
        self.eq_profiles.retain(|p| p.id != id);
        // Clear the active reference if we just deleted it.
        if matches!(prev_active, Some(EqProfileRef::User(active_id)) if active_id == id) {
            self.eq_active_profile = None;
        }
        self.eq_profile_delete_confirm = None;
        self.save_app_state();
    }

    /// Whether the currently active profile's stored values still
    /// match the live state. `false` means the user has tweaked the
    /// sliders since loading and the panel header should show a
    /// dirty `*` marker plus a "Save" button.
    pub(super) fn eq_active_profile_dirty(&self) -> bool {
        let Some(active) = self.eq_active_profile.as_ref() else {
            return false;
        };
        let live_gains = self.eq_state.gains_db();
        let live_preamp = self.eq_state.preamp_db();
        match active {
            EqProfileRef::Builtin(name) => match find_builtin_preset(name) {
                Some(preset) => {
                    !gains_match(&live_gains, &preset.gains_db)
                        || (live_preamp - preset.preamp_db).abs() > 0.01
                }
                None => true,
            },
            EqProfileRef::User(id) => match self.eq_profiles.iter().find(|p| p.id == *id) {
                Some(profile) => {
                    !gains_match(&live_gains, &profile.gains_db)
                        || (live_preamp - profile.preamp_db).abs() > 0.01
                }
                None => true,
            },
        }
    }

    /// Begin a vertical slider drag. Captures the band index, the
    /// pointer's start Y, and the current band gain so subsequent
    /// `mouse_move` events can compute a delta-driven new value
    /// independent of the slider's element bounds.
    pub(super) fn begin_eq_slider_drag(&mut self, band: usize, start_y: f32, start_gain_db: f32) {
        self.eq_slider_drag = Some(EqSliderDrag {
            band,
            start_y,
            start_gain_db,
            track_height_px: EQ_SLIDER_HEIGHT,
        });
    }

    pub(super) fn drag_eq_slider(&mut self, current_y: f32) {
        let Some(drag) = self.eq_slider_drag else {
            return;
        };
        let dy = current_y - drag.start_y;
        // The slider goes from +12dB at the top to -12dB at the
        // bottom. So `dy > 0` (moving down) decreases gain.
        let total_db = 2.0 * BAND_GAIN_LIMIT_DB;
        let delta_db = -dy / drag.track_height_px * total_db;
        let mut new_gain = drag.start_gain_db + delta_db;
        // Snap to 0 dB within ±0.5 dB.
        if new_gain.abs() < 0.5 {
            new_gain = 0.0;
        }
        new_gain = new_gain.clamp(-BAND_GAIN_LIMIT_DB, BAND_GAIN_LIMIT_DB);
        self.eq_state.set_band_gain_db(drag.band, new_gain);
        // Drag-time saves are debounced by `save_app_state` already.
        self.save_app_state();
    }

    pub(super) fn end_eq_slider_drag(&mut self) -> bool {
        self.eq_slider_drag.take().is_some()
    }

    pub(super) fn open_eq_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = TextInputState::default();
        // Default name suggestion based on active profile or "Custom".
        let default_name = match self.eq_active_profile.as_ref() {
            Some(EqProfileRef::Builtin(n)) => format!("{n} (custom)"),
            Some(EqProfileRef::User(id)) => self
                .eq_profiles
                .iter()
                .find(|p| p.id == *id)
                .map(|p| format!("{} (custom)", p.name))
                .unwrap_or_else(|| "Custom".to_string()),
            None => "Custom".to_string(),
        };
        input.set_text(default_name);
        input.select_all();
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        self.eq_profile_save_as = Some(EqProfileSaveAs { input });
        self.eq_profile_save_as_focus_handle = Some(focus_handle);
    }

    pub(super) fn cancel_eq_save_as(&mut self) {
        self.eq_profile_save_as = None;
        self.eq_profile_save_as_focus_handle = None;
    }

    pub(super) fn commit_eq_save_as(&mut self) {
        let Some(state) = self.eq_profile_save_as.take() else {
            return;
        };
        self.eq_profile_save_as_focus_handle = None;
        let name = state.input.text().to_string();
        self.create_eq_profile_from_current(name);
    }

    pub(super) fn handle_eq_save_as_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;
        let Some(state) = self.eq_profile_save_as.as_mut() else {
            return;
        };
        let key = event.keystroke.key.as_str().to_lowercase();
        match key.as_str() {
            "enter" => {
                self.commit_eq_save_as();
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.cancel_eq_save_as();
                cx.stop_propagation();
                cx.notify();
            }
            "backspace" => {
                state.input.backspace(command);
                cx.stop_propagation();
                cx.notify();
            }
            "delete" => {
                state.input.delete(command);
                cx.stop_propagation();
                cx.notify();
            }
            "left" => {
                state.input.move_left(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "right" => {
                state.input.move_right(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "home" => {
                state.input.move_home(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "end" => {
                state.input.move_end(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "a" if command => {
                state.input.select_all();
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    if !text.is_empty() && !command {
                        state.input.insert(text);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }
        }
    }

    /// Render the equalizer trigger button used in the top header
    /// next to the Settings cog. Left-click toggles the EQ panel;
    /// right-click toggles bypass (quick on/off without opening the
    /// panel). Shows a small accent dot when the EQ is engaged so
    /// users can see at a glance whether it's active.
    pub(super) fn render_eq_header_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
        let active = !self.eq_state.bypass();
        // State indication is glyph-only: off uses the same default
        // icon color as the Settings cog, on uses the theme accent.
        // The enabled outline remains on the button chrome, while the
        // glyph itself carries the accent state.
        let glyph_color = if active {
            colors.accent
        } else {
            colors.text_muted
        };

        // 2D EQ glyph: three small vertical sliders with offset
        // thumbs. Tinted accent when EQ is engaged.
        let color = format!("#{:06x}", glyph_color);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24"><line x1="5" y1="4" x2="5" y2="20" stroke="{color}" stroke-width="1.5" stroke-linecap="round"/><line x1="12" y1="4" x2="12" y2="20" stroke="{color}" stroke-width="1.5" stroke-linecap="round"/><line x1="19" y1="4" x2="19" y2="20" stroke="{color}" stroke-width="1.5" stroke-linecap="round"/><rect x="2" y="13" width="6" height="3" rx="1" fill="{color}"/><rect x="9" y="7" width="6" height="3" rx="1" fill="{color}"/><rect x="16" y="15" width="6" height="3" rx="1" fill="{color}"/></svg>"#
        );
        let icon = img(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.into_bytes(),
        )))
        .w(px(16.0))
        .h(px(16.0))
        .into_any_element();

        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(if active {
                colors.accent
            } else {
                colors.waveform_border
            }))
            .bg(rgb(colors.button))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |this| this.bg(rgb(colors.button_hover)))
            .active(|this| this.opacity(0.82))
            .child(icon)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _w, cx| {
                    this.toggle_eq_panel(event.position);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseDownEvent, _w, cx| {
                    // Quick bypass toggle — no panel, no UI churn.
                    this.toggle_eq_bypass();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    pub(super) fn render_eq_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let bypass = self.eq_state.bypass();
        let preamp_db = self.eq_state.preamp_db();
        let gains = self.eq_state.gains_db();
        let active_label = self.active_eq_profile_label();
        let dirty = self.eq_active_profile_dirty();

        let header = self.render_eq_header(active_label, dirty, bypass, cx);
        let preamp_row = self.render_eq_preamp_row(preamp_db, cx);
        let sliders = self.render_eq_sliders(&gains, cx);
        let footer = self.render_eq_footer(cx);

        let panel = menu_panel(EQ_PANEL_W, colors)
            .flex()
            .flex_col()
            .child(header)
            .child(preamp_row)
            .child(sliders)
            .child(footer);

        let panel = match self.eq_profile_save_as.as_ref() {
            Some(_) => panel.child(self.render_eq_save_as_row(cx)),
            None => panel,
        };

        let panel = match self.eq_profile_delete_confirm.as_ref() {
            Some(id) => panel.child(self.render_eq_delete_confirm(id, cx)),
            None => panel,
        };

        // Stop click-propagation from inside the panel so the
        // app-level dismiss handler doesn't immediately close the
        // panel on the same mousedown. Also close the profile-picker
        // dropdown if it's open: a click anywhere on the panel that
        // *isn't* on a picker item should dismiss the picker (the
        // picker itself stops propagation so clicks on its items
        // never reach this handler).
        let panel = panel.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                if this.eq_profile_menu_open {
                    this.eq_profile_menu_open = false;
                    cx.notify();
                }
                cx.stop_propagation();
            }),
        );

        // The profile-picker dropdown is rendered separately at root
        // level (see `render_eq_profile_menu_overlay`) so it floats
        // above the panel instead of being clipped by it. Other
        // context menus (column menu, queue context menu, etc.) use
        // the same pattern.

        // Anchored from `self.eq_panel_anchor` (the click position on
        // the EQ header button, which sits in the top-right of the
        // window next to the Settings cog). `TopRight` so the panel
        // *drops down and to the left* from the click, keeping it
        // on screen. The window-snap margin protects against the
        // panel running off the side on small windows.
        menu_at(
            self.eq_panel_anchor,
            Corner::TopRight,
            point(px(0.0), px(8.0)),
            panel,
        )
    }

    /// Profile-picker dropdown rendered at root level so it floats
    /// above the EQ panel without being clipped. Anchored from the
    /// same trigger point as the panel; the dropdown drops down to
    /// land on top of the panel header where the picker button sits.
    pub(super) fn render_eq_profile_menu_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.render_eq_profile_menu(cx)
    }

    fn active_eq_profile_label(&self) -> String {
        match self.eq_active_profile.as_ref() {
            Some(EqProfileRef::Builtin(n)) => n.clone(),
            Some(EqProfileRef::User(id)) => self
                .eq_profiles
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Custom".to_string()),
            None => "Custom".to_string(),
        }
    }

    fn render_eq_header(
        &self,
        active_label: String,
        dirty: bool,
        bypass: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        let active_user = matches!(self.eq_active_profile, Some(EqProfileRef::User(_)));
        let label_text = if dirty {
            format!("{active_label} *")
        } else {
            active_label
        };

        div()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("eq-on-off")
                    .cursor_pointer()
                    .h(px(22.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if bypass {
                        colors.waveform_border
                    } else {
                        colors.accent
                    }))
                    .bg(rgb(if bypass {
                        colors.button
                    } else {
                        colors.selected
                    }))
                    .text_color(rgb(if bypass {
                        colors.text_muted
                    } else {
                        colors.text_strong
                    }))
                    .font_weight(gpui::FontWeight::BOLD)
                    .flex()
                    .items_center()
                    .child(if bypass { "EQ Off" } else { "EQ On" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_eq_bypass();
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                // Profile picker dropdown trigger.
                div()
                    .id("eq-profile-picker")
                    .cursor_pointer()
                    .h(px(22.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_strong))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(label_text)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child("▾"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseDownEvent, _, cx| {
                            this.eq_profile_menu_open = !this.eq_profile_menu_open;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("eq-save")
                    .cursor_pointer()
                    .h(px(22.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(if active_user && dirty {
                        colors.text_strong
                    } else {
                        colors.text_faint
                    }))
                    .flex()
                    .items_center()
                    .child("Save")
                    .when(active_user && dirty, |this| {
                        this.on_click(cx.listener(|this, _, _, cx| {
                            this.save_eq_to_active_profile();
                            cx.notify();
                        }))
                    }),
            )
            .child(
                div()
                    .id("eq-save-as")
                    .cursor_pointer()
                    .h(px(22.0))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text))
                    .flex()
                    .items_center()
                    .child("Save as…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_eq_save_as(window, cx);
                        cx.notify();
                    })),
            )
    }

    fn render_eq_preamp_row(
        &self,
        preamp_db: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let normalized = ((preamp_db + PREAMP_LIMIT_DB) / (2.0 * PREAMP_LIMIT_DB)).clamp(0.0, 1.0);
        let bar_w = 200.0_f32;
        let fill_w = bar_w * normalized;

        div()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(56.0))
                    .text_color(rgb(colors.text_muted))
                    .child("Preamp"),
            )
            .child({
                div()
                    .id("eq-preamp-bar")
                    .w(px(bar_w))
                    .h(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _w, cx| {
                            // Click-to-jump: convert local x into a
                            // [-PREAMP_LIMIT_DB, +PREAMP_LIMIT_DB] dB
                            // value. We use the event position
                            // directly; it's relative to the click
                            // target's bounds in GPUI's hit-test
                            // model, which is good enough for a
                            // small horizontal slider.
                            let local_x = f32::from(event.position.x).max(0.0);
                            // The full bar is `bar_w`; clip and map.
                            let t = (local_x / bar_w).clamp(0.0, 1.0);
                            let db = -PREAMP_LIMIT_DB + t * 2.0 * PREAMP_LIMIT_DB;
                            // Snap to 0 dB.
                            let db = if db.abs() < 0.5 { 0.0 } else { db };
                            this.set_eq_preamp(db);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(colors.text_faint))
                            .child(
                                div()
                                    .w(px(fill_w))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgb(colors.accent)),
                            ),
                    )
            })
            .child(
                div()
                    .w(px(56.0))
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(format!("{:+.1} dB", preamp_db)),
            )
    }

    fn render_eq_sliders(
        &self,
        gains: &[f32; BAND_COUNT],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let mut row = div()
            .px_3()
            .py_3()
            .flex()
            .items_end()
            .justify_between()
            .gap_2();
        for band in 0..BAND_COUNT {
            row = row.child(self.render_eq_slider(band, gains[band], cx));
        }

        // Add the dB-axis tick labels on the left side as a sibling
        // for visual reference.
        div()
            .border_b_1()
            .border_color(rgb(colors.border))
            .flex()
            .child(
                div()
                    .w(px(36.0))
                    .py_3()
                    .pl_2()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(div().child("+12"))
                    .child(div().child("0"))
                    .child(div().child("-12")),
            )
            .child(div().flex_1().child(row))
    }

    fn render_eq_slider(
        &self,
        band: usize,
        gain_db: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let normalized =
            ((gain_db + BAND_GAIN_LIMIT_DB) / (2.0 * BAND_GAIN_LIMIT_DB)).clamp(0.0, 1.0);
        // Convert normalized [0..=1] into a thumb top offset from
        // the top of the track. 0dB -> midpoint; +12 -> 0; -12 ->
        // EQ_SLIDER_HEIGHT.
        let thumb_top = (1.0 - normalized) * EQ_SLIDER_HEIGHT - EQ_SLIDER_THUMB_H * 0.5;

        let dragging = self
            .eq_slider_drag
            .as_ref()
            .map(|d| d.band == band)
            .unwrap_or(false);
        let label = BAND_LABELS[band];

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .w(px(36.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_strong))
                    .child(format!("{:+.1}", gain_db)),
            )
            .child({
                // Slider track region.
                div()
                    .id(SharedString::from(format!("eq-slider-{band}")))
                    .relative()
                    .h(px(EQ_SLIDER_HEIGHT))
                    .w(px(EQ_SLIDER_THUMB_W + 6.0))
                    .flex()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _w, cx| {
                            // Click-to-jump and grab. Use the click's
                            // absolute Y as both the start_y and a
                            // basis for converting the offset within
                            // the track to a starting gain. Simpler
                            // model: we compute the gain *at the
                            // click point* and seed the drag from
                            // there so the thumb instantly snaps to
                            // the cursor and follows it.
                            let click_y = f32::from(event.position.y);
                            // For seeding we read back the band gain
                            // *after* applying the click-jump so the
                            // drag math stays consistent.
                            let local_y = f32::from(event.position.y);
                            // Estimate a click-to-gain by remembering
                            // that the slider track sits inside a
                            // taller column (label on top). We treat
                            // the click as adjusting from the
                            // existing gain by clamping a derived
                            // value -- but simplest: just begin a
                            // drag with no initial jump. The user's
                            // mouse-move will move the thumb. This
                            // matches the volume-bar pattern in the
                            // player.
                            let _ = local_y;
                            let start_gain = this.eq_state.band_gain_db(band);
                            this.begin_eq_slider_drag(band, click_y, start_gain);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _w, cx| {
                        if this
                            .eq_slider_drag
                            .as_ref()
                            .map(|d| d.band == band)
                            .unwrap_or(false)
                        {
                            this.drag_eq_slider(f32::from(event.position.y));
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                            if this.end_eq_slider_drag() {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    // Dbl-click resets to 0 dB.
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        if event.click_count() >= 2 {
                            this.set_eq_band_gain(band, 0.0);
                            cx.notify();
                        }
                    }))
                    // Track.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .w(px(EQ_SLIDER_TRACK_W))
                            .h(px(EQ_SLIDER_HEIGHT))
                            .rounded_full()
                            .bg(rgb(colors.text_faint)),
                    )
                    // 0 dB centerline.
                    .child(
                        div()
                            .absolute()
                            .top(px(EQ_SLIDER_HEIGHT * 0.5 - 1.0))
                            .w(px(EQ_SLIDER_THUMB_W + 4.0))
                            .h(px(1.0))
                            .bg(rgb(colors.border)),
                    )
                    // Thumb.
                    .child(
                        div()
                            .absolute()
                            .top(px(thumb_top.clamp(
                                -EQ_SLIDER_THUMB_H * 0.5,
                                EQ_SLIDER_HEIGHT - EQ_SLIDER_THUMB_H * 0.5,
                            )))
                            .w(px(EQ_SLIDER_THUMB_W))
                            .h(px(EQ_SLIDER_THUMB_H))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if dragging {
                                colors.accent
                            } else {
                                colors.waveform_border
                            }))
                            .bg(rgb(if dragging {
                                colors.selected
                            } else {
                                colors.button
                            })),
                    )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(label),
            )
    }

    fn render_eq_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("eq-reset")
                    .cursor_pointer()
                    .h(px(24.0))
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text))
                    .hover(move |this| {
                        this.bg(rgb(colors.button_hover))
                            .text_color(rgb(colors.text_strong))
                    })
                    .flex()
                    .items_center()
                    .child("Reset to Flat")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reset_eq_to_flat();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child("Click a band to drag · double-click resets · ±12 dB"),
            )
    }

    fn render_eq_save_as_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let Some(state) = self.eq_profile_save_as.as_ref() else {
            return div().into_any_element();
        };
        let Some(focus_handle) = self.eq_profile_save_as_focus_handle.as_ref() else {
            return div().into_any_element();
        };
        let text = state.input.text().to_string();
        let cursor = state.input.cursor();
        let selection = state.input.selection_range();

        let mut children: Vec<AnyElement> = Vec::new();
        if let Some(range) = selection {
            if range.start > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(text[..range.start].to_string())
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .flex_none()
                    .rounded_sm()
                    .bg(rgb(colors.selected))
                    .text_color(rgb(colors.text_strong))
                    .child(text[range.clone()].to_string())
                    .into_any_element(),
            );
            if range.end < text.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(text[range.end..].to_string())
                        .into_any_element(),
                );
            }
        } else {
            if cursor > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(text[..cursor].to_string())
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .flex_none()
                    .w(px(1.0))
                    .h(px(14.0))
                    .bg(rgb(colors.text_strong))
                    .into_any_element(),
            );
            if cursor < text.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(text[cursor..].to_string())
                        .into_any_element(),
                );
            }
        }

        div()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_color(rgb(colors.text_muted)).child("Save as"))
            .child(
                div()
                    .id("eq-save-as-input")
                    .min_w_0()
                    .flex_1()
                    .h(px(22.0))
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(colors.accent))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_strong))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .track_focus(focus_handle)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                        this.handle_eq_save_as_key_down(event, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .children(children),
            )
            .child(
                div()
                    .id("eq-save-as-confirm")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.accent))
                    .bg(rgb(colors.selected))
                    .text_color(rgb(colors.text_strong))
                    .child("Save")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commit_eq_save_as();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("eq-save-as-cancel")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text))
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel_eq_save_as();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_eq_delete_confirm(
        &self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let name = self
            .eq_profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let id_owned = id.to_string();
        div()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.elevated))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(colors.text))
                    .child(format!("Delete profile \"{name}\"?")),
            )
            .child(
                div()
                    .id("eq-delete-confirm")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.accent))
                    .bg(rgb(colors.selected))
                    .text_color(rgb(colors.text_strong))
                    .child("Delete")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.delete_eq_profile(&id_owned);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("eq-delete-cancel")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text))
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.eq_profile_delete_confirm = None;
                        cx.notify();
                    })),
            )
    }

    fn render_eq_profile_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        let mut items: Vec<AnyElement> = Vec::new();
        items.push(menu_section_label("Built-in", colors).into_any_element());
        for preset in BUILTIN_PRESETS {
            let name_owned = preset.name.to_string();
            let active = matches!(
                self.eq_active_profile.as_ref(),
                Some(EqProfileRef::Builtin(n)) if n.eq_ignore_ascii_case(preset.name)
            );
            let label = if active {
                format!("✓  {}", preset.name)
            } else {
                format!("    {}", preset.name)
            };
            let id_str = format!(
                "eq-builtin-{}",
                preset.name.to_ascii_lowercase().replace(' ', "-")
            );
            items.push(
                menu_item(SharedString::from(id_str), label, colors)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_eq_profile(&EqProfileRef::Builtin(name_owned.clone()));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        if !self.eq_profiles.is_empty() {
            items.push(menu_section_label("Your profiles", colors).into_any_element());
            for profile in &self.eq_profiles {
                let id = profile.id.clone();
                let id_for_load = id.clone();
                let id_for_delete = id.clone();
                let name = profile.name.clone();
                let active = matches!(
                    self.eq_active_profile.as_ref(),
                    Some(EqProfileRef::User(n)) if n == &profile.id
                );
                let label = if active {
                    format!("✓  {name}")
                } else {
                    format!("    {name}")
                };
                items.push(
                    div()
                        .id(SharedString::from(format!("eq-user-row-{id}")))
                        .h(px(28.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(rgb(colors.text))
                        .hover(move |this| {
                            this.bg(rgb(colors.button_hover))
                                .text_color(rgb(colors.text_strong))
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("eq-user-load-{id_for_load}")))
                                .cursor_pointer()
                                .flex_1()
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.load_eq_profile(&EqProfileRef::User(id_for_load.clone()));
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "eq-user-delete-{id_for_delete}"
                                )))
                                .cursor_pointer()
                                .px_2()
                                .text_xs()
                                .text_color(rgb(colors.text_muted))
                                .child("Delete")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.eq_profile_delete_confirm = Some(id_for_delete.clone());
                                    this.eq_profile_menu_open = false;
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                );
            }
        }

        // The trigger button sits in the top-right of the window;
        // the panel drops down from there, with the profile picker
        // in the panel's header row. We anchor the dropdown from
        // the same trigger point with a `TopRight` corner so it
        // floats *below* the trigger and *to the left*, landing
        // roughly on top of the panel header. The exact offset
        // (~50px down from the click) accounts for the panel's
        // 8px gap + ~40px header height. A more precise anchoring
        // would need the picker button to expose its painted rect.
        menu_at(
            self.eq_panel_anchor,
            Corner::TopRight,
            point(px(0.0), px(50.0)),
            menu_panel(220.0, colors).children(items),
        )
    }
}

fn gains_match(a: &[f32; BAND_COUNT], b: &[f32; BAND_COUNT]) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(left, right)| (left - right).abs() < 0.01)
}
