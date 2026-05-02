use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::*;

/// Process-wide cache of pre-rasterized Settings nav icons. Mirrors
/// the sidebar's icon cache (`sidebar.rs::sidebar_icon_cache`):
/// keyed by `(section, active, color, accent)` so theme switches
/// and the active/inactive flip both invalidate only the affected
/// entries while every subsequent render is a cheap `Arc<Image>`
/// clone instead of a fresh SVG encode.
type SettingsIconCacheKey = (SettingsSection, bool, u32, u32);
fn settings_icon_cache() -> &'static Mutex<HashMap<SettingsIconCacheKey, Arc<Image>>> {
    static CACHE: OnceLock<Mutex<HashMap<SettingsIconCacheKey, Arc<Image>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl TempoApp {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();
        let active_section = self.settings_section;
        let show_onboarding =
            self.library_roots.is_empty() && active_section == SettingsSection::Library;

        div()
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .h(px(54.0))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(self.left_sidebar_collapsed, |this| {
                                this.child(self.sidebar_button("›", "open-left-sidebar").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.left_sidebar_collapsed = false;
                                        cx.notify();
                                    }),
                                ))
                            })
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_strong))
                                    .child(if self.library_roots.is_empty() {
                                        "Set Up Tempo"
                                    } else {
                                        "Settings"
                                    }),
                            ),
                    )
                    .when(!self.library_roots.is_empty(), |this| {
                        this.child(
                            div()
                                .id("settings-back")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors.waveform_border))
                                .bg(rgb(colors.button))
                                .active(|this| this.opacity(0.82))
                                .child("Back to Library")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_page(Page::Library);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            // Two-pane body: left nav (pagination, not scroll-spy)
            // + right detail pane that renders only the active
            // section. Each pane scrolls independently when its
            // content overflows the window height.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_settings_nav(cx))
                    .child(
                        div()
                            .id("settings-detail-scroll")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .when(show_onboarding, |this| {
                                this.child(self.render_onboarding_card())
                            })
                            .child(match active_section {
                                SettingsSection::Appearance => {
                                    self.render_theme_settings(cx).into_any_element()
                                }
                                SettingsSection::AudioOutput => {
                                    self.render_output_settings(cx).into_any_element()
                                }
                                SettingsSection::Window => {
                                    self.render_window_settings(cx).into_any_element()
                                }
                                SettingsSection::Library => {
                                    self.render_library_settings(cx).into_any_element()
                                }
                                SettingsSection::OnlineMetadata => {
                                    self.render_online_metadata_settings(cx).into_any_element()
                                }
                                SettingsSection::Hotkeys => {
                                    super::hotkeys_panel::render_hotkey_section(self, cx)
                                }
                            }),
                    ),
            )
    }

    /// Left-pane section nav for the Settings page. Pagination-style:
    /// clicking an entry sets `self.settings_section` and the right
    /// pane swaps content.
    ///
    /// Density and styling match the left sidebar's `render_nav_item`
    /// (`h=22px`, `px_2`, `gap_2`, icon + label) — the Settings nav
    /// is conceptually the same kind of list, so it should *feel*
    /// identical: small SVG glyphs, tight rows, hover/active states
    /// that mirror the sidebar.
    pub(super) fn render_settings_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let active = self.settings_section;

        div()
            .id("settings-nav")
            .flex_none()
            .w(px(180.0))
            .min_h_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .py_3()
            .px_3()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                SettingsSection::all()
                    .into_iter()
                    .map(|section| self.render_settings_nav_item(section, active, cx)),
            )
    }

    fn render_settings_nav_item(
        &self,
        section: SettingsSection,
        active: SettingsSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let selected = section == active;
        let label = section.label();
        let id = SharedString::from(format!(
            "settings-nav-{}",
            label.to_ascii_lowercase().replace(' ', "-")
        ));

        let bg = if selected {
            colors.button_hover
        } else {
            colors.panel
        };
        let fg = if selected {
            colors.text_strong
        } else {
            colors.text
        };

        div()
            .id(id)
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .when(!selected, |this| {
                this.hover(move |this| {
                    this.bg(rgb(colors.button_hover))
                        .text_color(rgb(colors.text_strong))
                })
            })
            .active(|this| this.opacity(0.82))
            .child(Self::settings_nav_icon(section, selected, colors))
            .child(div().overflow_hidden().text_ellipsis().child(label))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.settings_section != section {
                    this.settings_section = section;
                    cx.notify();
                }
            }))
    }

    /// Produce a 15px SVG glyph for a Settings section. Cached
    /// process-wide via `settings_icon_cache` so each unique
    /// `(section, active, color, accent)` combination only encodes
    /// once — same approach as `sidebar_nav_icon`.
    fn settings_nav_icon(
        section: SettingsSection,
        active: bool,
        colors: ThemeColors,
    ) -> AnyElement {
        let color_u32 = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let accent_u32 = colors.accent;
        let cache_key = (section, active, color_u32, accent_u32);

        if let Ok(cache) = settings_icon_cache().lock()
            && let Some(image) = cache.get(&cache_key)
        {
            return img(Arc::clone(image))
                .w(px(15.0))
                .h(px(15.0))
                .flex_none()
                .into_any_element();
        }

        let color = format!("#{:06x}", color_u32);
        let accent = format!("#{:06x}", accent_u32);
        let accent_stroke = if active {
            accent.as_str()
        } else {
            color.as_str()
        };

        let paths = match section {
            // Paint palette / brush — appearance & theming.
            SettingsSection::Appearance => format!(
                r#"<path d="M12 4.2C7.6 4.2 4 7.4 4 11.4C4 14.4 6.2 16.5 8.6 16.5C9.6 16.5 10.2 16.1 10.2 15.3C10.2 14.9 10 14.6 9.7 14.2C9.4 13.8 9.2 13.5 9.2 13.1C9.2 12.4 9.7 12 10.5 12H12.5C16.2 12 19 9.8 19 7.4C19 5.4 16.2 4.2 12 4.2Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<circle cx="7.5" cy="9" r="1.1" fill="{color}"/>
<circle cx="10.7" cy="6.7" r="1.1" fill="{accent_stroke}"/>
<circle cx="14.2" cy="6.7" r="1.1" fill="{color}"/>
<circle cx="16.6" cy="9.5" r="1.1" fill="{accent_stroke}"/>"#
            ),
            // Speaker / output device.
            SettingsSection::AudioOutput => format!(
                r#"<path d="M5 9.5H8.2L13 5.5V18.5L8.2 14.5H5V9.5Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M16 9.2C17.2 10.2 17.9 11.5 17.9 12.9C17.9 14.4 17.2 15.7 16 16.7" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>
<path d="M18.5 6.8C20.4 8.4 21.4 10.6 21.4 12.9C21.4 15.3 20.4 17.5 18.5 19.1" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>"#
            ),
            // Window / workspace restore behaviour.
            SettingsSection::Window => format!(
                r#"<rect x="4" y="5" width="16" height="14" rx="2" fill="none" stroke="{color}" stroke-width="1.6"/>
<path d="M4 9H20" fill="none" stroke="{color}" stroke-width="1.5"/>
<path d="M8 14H15.5" fill="none" stroke="{accent_stroke}" stroke-width="1.6" stroke-linecap="round"/>
<path d="M13.2 11.8L15.5 14L13.2 16.2" fill="none" stroke="{accent_stroke}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#
            ),
            // Folder — library roots.
            SettingsSection::Library => format!(
                r#"<path d="M3.5 7.2C3.5 6.4 4.1 5.8 4.9 5.8H9.6L11.1 7.6H19.1C19.9 7.6 20.5 8.2 20.5 9V17.4C20.5 18.2 19.9 18.8 19.1 18.8H4.9C4.1 18.8 3.5 18.2 3.5 17.4V7.2Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M7 11.8H17" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>
<path d="M7 14.6H13.5" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>"#
            ),
            // Globe / cloud — online metadata.
            SettingsSection::OnlineMetadata => format!(
                r#"<circle cx="12" cy="12" r="7.6" fill="none" stroke="{color}" stroke-width="1.6"/>
<path d="M4.4 12H19.6" fill="none" stroke="{color}" stroke-width="1.5"/>
<path d="M12 4.4C14.2 6.5 15.4 9.2 15.4 12C15.4 14.8 14.2 17.5 12 19.6C9.8 17.5 8.6 14.8 8.6 12C8.6 9.2 9.8 6.5 12 4.4Z" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linejoin="round"/>"#
            ),
            // Keyboard — global hotkeys.
            SettingsSection::Hotkeys => format!(
                r#"<rect x="3" y="6.5" width="18" height="11" rx="1.6" fill="none" stroke="{color}" stroke-width="1.6"/>
<rect x="5.6" y="9" width="1.6" height="1.6" fill="{color}"/>
<rect x="8.4" y="9" width="1.6" height="1.6" fill="{accent_stroke}"/>
<rect x="11.2" y="9" width="1.6" height="1.6" fill="{color}"/>
<rect x="14.0" y="9" width="1.6" height="1.6" fill="{color}"/>
<rect x="16.8" y="9" width="1.6" height="1.6" fill="{accent_stroke}"/>
<rect x="7.5" y="13.6" width="9" height="1.6" fill="{accent_stroke}"/>"#
            ),
        };

        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">{paths}</svg>"#
        );

        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        if let Ok(mut cache) = settings_icon_cache().lock() {
            cache.insert(cache_key, Arc::clone(&image));
        }

        img(image)
            .w(px(15.0))
            .h(px(15.0))
            .flex_none()
            .into_any_element()
    }

    pub(super) fn render_onboarding_card(&self) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .bg(rgb(colors.elevated))
            .p_5()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child("Choose where Tempo should scan"),
            )
            .child(
                div()
                    .text_color(rgb(colors.text_muted))
                    .child("Add one or more music folders to start indexing your local library."),
            )
    }

    pub(super) fn render_theme_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();
        let theme_options = self
            .themes
            .iter()
            .map(|theme| self.render_theme_option(theme, cx))
            .collect::<Vec<_>>();

        // No `max_h` / inner scroll here — the right-pane scroll on
        // the Settings page handles overflow when there are many
        // themes. Constraining height here would create a nested
        // scroll inside an already-scrolling container.
        div()
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Theme"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(self.theme().name.clone()),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(theme_options),
            )
    }

    pub(super) fn render_theme_option(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let theme_colors = theme.colors;
        let selected = self.theme_id == theme.id;
        let theme_id = theme.id.clone();
        let swatches = [
            theme_colors.app,
            theme_colors.surface,
            theme_colors.elevated,
            theme_colors.accent,
            theme_colors.text_strong,
        ];

        div()
            .id(SharedString::from(format!("theme-option-{}", theme.id)))
            .min_h(px(58.0))
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.border
            }))
            .bg(rgb(if selected {
                colors.selected
            } else {
                colors.button
            }))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_3()
            .hover(move |this| {
                this.bg(rgb(if selected {
                    colors.selected
                } else {
                    colors.hover
                }))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .child(theme.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(theme.description.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(swatches.into_iter().map(|swatch| {
                        div()
                            .w(px(18.0))
                            .h(px(18.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(colors.border_strong))
                            .bg(rgb(swatch))
                    })),
            )
            .child(
                div()
                    .w(px(56.0))
                    .text_xs()
                    .text_color(rgb(if selected {
                        colors.accent_soft
                    } else {
                        colors.text_faint
                    }))
                    .child(if selected { "Active" } else { "" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_theme(&theme_id, cx);
                cx.notify();
            }))
    }

    pub(super) fn render_output_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Audio Output"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.player.read(cx).current_output_label()),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(rgb(colors.text_strong))
                                    .child("Playback device"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors.text_muted))
                                    .child("Choose where Tempo sends audio."),
                            ),
                    )
                    .child(self.playback_status_dropdown(OutputMenuSource::Settings, cx)),
            )
    }

    pub(super) fn render_window_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Window"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(self.window_restore_mode.label()),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .child("When opening Tempo from another workspace"),
                    )
                    .child(self.render_window_restore_option(
                        WindowRestoreMode::BringHere,
                        "Move Tempo to the current workspace as a floating window.",
                        cx,
                    ))
                    .child(self.render_window_restore_option(
                        WindowRestoreMode::GoToWindow,
                        "Switch to the workspace where Tempo already is.",
                        cx,
                    )),
            )
    }

    fn render_window_restore_option(
        &self,
        mode: WindowRestoreMode,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let selected = self.window_restore_mode == mode;

        div()
            .id(SharedString::from(format!(
                "window-restore-{}",
                match mode {
                    WindowRestoreMode::BringHere => "bring-here",
                    WindowRestoreMode::GoToWindow => "go-to-window",
                }
            )))
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.border
            }))
            .bg(rgb(if selected {
                colors.selected
            } else {
                colors.button
            }))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .hover(move |this| {
                this.bg(rgb(if selected {
                    colors.selected
                } else {
                    colors.hover
                }))
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .child(mode.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .w(px(56.0))
                    .text_xs()
                    .text_color(rgb(if selected {
                        colors.accent_soft
                    } else {
                        colors.text_faint
                    }))
                    .child(if selected { "Active" } else { "" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.window_restore_mode != mode {
                    this.window_restore_mode = mode;
                    this.save_app_state();
                    cx.notify();
                }
            }))
    }

    pub(super) fn render_library_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Library"))
                    .child(
                        self.settings_button("Add folder", "add-library-folder")
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                let paths = cx.prompt_for_paths(PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: true,
                                    prompt: Some("Choose music folders".into()),
                                });

                                cx.spawn(async move |this, cx| {
                                    if let Ok(Ok(Some(paths))) = paths.await {
                                        let _ = this.update(cx, |app, cx| {
                                            app.add_library_roots(paths, cx);
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            })),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if self.is_scanning {
                                colors.accent
                            } else {
                                colors.text_muted
                            }))
                            .child(self.visible_scan_status()),
                    )
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(div().text_color(rgb(colors.text)).child(
                            "No folders configured. Use Add folder to choose one or more roots.",
                        ))
                    })
                    .children(
                        self.library_roots
                            .iter()
                            .enumerate()
                            .map(|(ix, root)| self.render_library_root_row(ix, root, cx)),
                    )
                    .when(PathBuf::from("/mnt/data/music").is_dir(), |this| {
                        this.child(
                            self.settings_button("Add /mnt/data/music", "add-mounted-music")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.add_library_roots(
                                        vec![PathBuf::from("/mnt/data/music")],
                                        cx,
                                    );
                                    cx.notify();
                                })),
                        )
                    }),
            )
    }

    pub(super) fn render_online_metadata_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Online Metadata"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(self.online_metadata_mode.label()),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(
                                "Optional. Automatic mode contacts MusicBrainz to resolve artist IDs for future profile and discography enrichment.",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.render_online_metadata_option(OnlineMetadataMode::Off, cx))
                            .child(self.render_online_metadata_option(
                                OnlineMetadataMode::Automatic,
                                cx,
                            )),
                    )
                    .when(self.online_metadata_mode == OnlineMetadataMode::Automatic, |this| {
                        this.child(self.render_online_metadata_resync_row(cx))
                    }),
            )
    }

    /// Manual "Resync metadata" action surfaced as a button row inside
    /// the Online Metadata settings panel. Walks every artist/album
    /// that's still missing a bio, photo, description, or cover and
    /// enqueues the next link in the multi-source fallback chain.
    /// Useful after a fresh build that adds new sources (Wikipedia /
    /// Discogs) lands on a library that previously got stuck on
    /// TheAudioDB-only enrichment.
    fn render_online_metadata_resync_row(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let activity = &self.metadata_activity;
        let status_label = if activity.is_active() {
            format!(
                "Syncing… {} active, {} queued",
                activity.running.max(1),
                activity.pending,
            )
        } else if let Some(reported) = self.metadata_resync_status.as_deref() {
            reported.to_string()
        } else {
            "Idle".to_string()
        };

        div()
            .pt_2()
            .border_t_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_strong))
                            .child("Resync metadata"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(status_label),
                    ),
            )
            .child(
                div()
                    .id("online-metadata-resync")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text))
                    .hover(|this| this.bg(rgb(colors.button_hover)))
                    .active(|this| this.opacity(0.82))
                    .child("Resync now")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_metadata_resync();
                        cx.notify();
                    })),
            )
    }

    fn render_online_metadata_option(
        &self,
        mode: OnlineMetadataMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let selected = self.online_metadata_mode == mode;

        div()
            .id(SharedString::from(format!(
                "online-metadata-{}",
                mode.label().to_ascii_lowercase()
            )))
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.waveform_border
            }))
            .bg(rgb(if selected {
                colors.selected
            } else {
                colors.button
            }))
            .text_color(rgb(if selected {
                colors.text_strong
            } else {
                colors.text
            }))
            .hover(move |this| {
                this.bg(rgb(if selected {
                    colors.selected
                } else {
                    colors.button_hover
                }))
            })
            .active(|this| this.opacity(0.82))
            .child(mode.label())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_online_metadata_mode(mode);
                cx.notify();
            }))
    }

    pub(super) fn render_library_root_row(
        &self,
        ix: usize,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let root_label = root.display().to_string();
        let colors = *self.colors();

        div()
            .min_h(px(34.0))
            .px_3()
            .rounded_md()
            .bg(rgb(colors.row))
            .border_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(colors.text))
                    .child(root_label),
            )
            .child(
                self.settings_button("Remove", format!("remove-library-root-{ix}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_library_root(ix, cx);
                        cx.notify();
                    })),
            )
    }

    pub(super) fn settings_button(
        &self,
        label: &'static str,
        id: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();
        let colors = *self.colors();

        div()
            .id(id)
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .bg(rgb(colors.button))
            .text_color(rgb(colors.text))
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .text_color(rgb(colors.text_strong))
            })
            .active(|this| this.opacity(0.82))
            .child(label)
    }
}
