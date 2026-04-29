use std::path::Path;

use super::*;

impl TempoApp {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();

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
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(self.render_onboarding_card())
                    })
                    .child(self.render_theme_settings(cx))
                    .child(self.render_output_settings(cx))
                    .child(self.render_online_metadata_settings(cx))
                    .child(self.render_library_settings(cx))
                    .child(self.render_playlist_settings(cx)),
            )
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

        div()
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .max_h(px(380.0))
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
                    .id("theme-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
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
                this.set_theme(&theme_id);
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
                            .child(self.current_output_label()),
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
                    ),
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

    pub(super) fn render_playlist_settings(
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
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Playlists"))
                    .child(
                        self.settings_button("New playlist", "new-playlist-settings")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
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
                    .when(self.playlists.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(rgb(colors.text))
                                .child("No playlists yet. Create one to start organizing tracks."),
                        )
                    })
                    .children(
                        self.playlists
                            .iter()
                            .enumerate()
                            .map(|(ix, playlist)| self.render_playlist_settings_row(ix, playlist)),
                    ),
            )
    }

    pub(super) fn render_playlist_settings_row(
        &self,
        ix: usize,
        playlist: &Playlist,
    ) -> impl IntoElement + use<> {
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
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(colors.text))
                    .child(playlist.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(format!("{} tracks", playlist.track_paths.len())),
            )
            .id(SharedString::from(format!("settings-playlist-{ix}")))
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
