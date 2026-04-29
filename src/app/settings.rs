use super::*;

impl TempoApp {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .bg(rgb(0x131419))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(54.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(0x24252b))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(self.left_sidebar_collapsed, |this| {
                                this.child(Self::sidebar_button("›", "open-left-sidebar").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.left_sidebar_collapsed = false;
                                        cx.notify();
                                    }),
                                ))
                            })
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(0xf0f0f4))
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
                                .border_color(rgb(0x30323a))
                                .bg(rgb(0x1b1c22))
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
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(self.render_onboarding_card())
                    })
                    .child(self.render_library_settings(cx))
                    .child(self.render_playlist_settings(cx)),
            )
    }

    pub(super) fn render_onboarding_card(&self) -> impl IntoElement {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343741))
            .bg(rgb(0x1b1c22))
            .p_5()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .child("Choose where Tempo should scan"),
            )
            .child(
                div()
                    .text_color(rgb(0xa6aab4))
                    .child("Add one or more music folders to start indexing your local library."),
            )
    }

    pub(super) fn render_library_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17181e))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x1b1c22))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Library"))
                    .child(
                        Self::settings_button("Add folder", "add-library-folder").on_click(
                            cx.listener(|_this, _, _window, cx| {
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
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(0x24252b))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if self.is_scanning { 0xeeb17d } else { 0x858993 }))
                            .child(self.visible_scan_status()),
                    )
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(div().text_color(rgb(0xc9ccd4)).child(
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
                            Self::settings_button("Add /mnt/data/music", "add-mounted-music")
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

    pub(super) fn render_library_root_row(
        &self,
        ix: usize,
        root: &PathBuf,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let root_label = root.display().to_string();
        div()
            .min_h(px(34.0))
            .px_3()
            .rounded_md()
            .bg(rgb(0x131419))
            .border_1()
            .border_color(rgb(0x24252b))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(0xd8d8dd))
                    .child(root_label),
            )
            .child(
                Self::settings_button("Remove", &format!("remove-library-root-{ix}")).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.remove_library_root(ix, cx);
                        cx.notify();
                    }),
                ),
            )
    }

    pub(super) fn render_playlist_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17181e))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x1b1c22))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Playlists"))
                    .child(
                        Self::settings_button("New playlist", "new-playlist-settings").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(0x24252b))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(self.playlists.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(rgb(0xc9ccd4))
                                .child("No playlists yet. Create one to start organizing tracks."),
                        )
                    })
                    .children(
                        self.playlists
                            .iter()
                            .enumerate()
                            .map(|(ix, playlist)| Self::render_playlist_settings_row(ix, playlist)),
                    ),
            )
    }

    pub(super) fn render_playlist_settings_row(
        ix: usize,
        playlist: &Playlist,
    ) -> impl IntoElement + use<> {
        div()
            .min_h(px(34.0))
            .px_3()
            .rounded_md()
            .bg(rgb(0x131419))
            .border_1()
            .border_color(rgb(0x24252b))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(0xd8d8dd))
                    .child(playlist.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x858993))
                    .child(format!("{} tracks", playlist.track_paths.len())),
            )
            .id(SharedString::from(format!("settings-playlist-{ix}")))
    }

    pub(super) fn settings_button(
        label: &'static str,
        id: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();

        div()
            .id(id)
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x30323a))
            .bg(rgb(0x1b1c22))
            .text_color(rgb(0xc9ccd4))
            .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
            .active(|this| this.opacity(0.82))
            .child(label)
    }
}
