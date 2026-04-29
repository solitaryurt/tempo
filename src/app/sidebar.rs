use super::*;

impl TempoApp {
    pub(super) fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.left_sidebar_collapsed;

        if collapsed {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        div()
            .w(px(LEFT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x15161a))
            .child(
                div()
                    .w(px(LEFT_SIDEBAR_W))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.render_sidebar_header(cx))
                    .child(self.render_library_nav(cx))
                    .child(self.render_playlists_nav(cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(0x24252b))
                            .text_xs()
                            .text_color(rgb(0x6f737c))
                            .flex()
                            .justify_between()
                            .child(format!("{} tracks", self.tracks.len()))
                            .child(Self::format_library_size(&self.tracks)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(50.0))
            .flex()
            .items_center()
            .px_4()
            .border_b_1()
            .border_color(rgb(0x1e2026))
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .child("Tempo"),
            )
            .child(
                Self::sidebar_button("‹", "toggle-left-sidebar").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.left_sidebar_collapsed = !this.left_sidebar_collapsed;
                        cx.notify();
                    },
                )),
            )
    }

    pub(super) fn sidebar_button(
        label: &'static str,
        id: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x30323a))
            .bg(rgb(0x1b1c22))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x9a9ea8))
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    pub(super) fn render_library_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::nav_group_title("LIBRARY"))
            .child(self.render_nav_item(
                "All Music",
                self.tracks.len().to_string(),
                self.page == Page::Library && self.active_tab().source == TabSource::Library,
                Page::Library,
                cx,
            ))
            .child(self.render_nav_item(
                "Settings",
                "",
                self.page == Page::Settings,
                Page::Settings,
                cx,
            ))
    }

    pub(super) fn render_playlists_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(Self::nav_group_title("PLAYLISTS"))
                    .child(
                        Self::sidebar_button("+", "new-playlist").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(self.playlists.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(0x777b84))
                        .child("No playlists yet"),
                )
            })
            .children(
                self.playlists
                    .iter()
                    .enumerate()
                    .map(|(ix, playlist)| self.render_playlist_nav_item(ix, playlist, cx)),
            )
    }

    pub(super) fn nav_group_title(title: &'static str) -> impl IntoElement {
        div()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(0x666a73))
            .child(title)
    }

    pub(super) fn render_playlist_nav_item(
        &self,
        ix: usize,
        playlist: &Playlist,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active =
            self.page == Page::Library && self.active_tab().source == TabSource::Playlist(ix);
        let bg = if active { 0x282a30 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0xb6b8bf };

        div()
            .id(SharedString::from(format!("playlist-{ix}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .active(|this| this.opacity(0.82))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(playlist.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(playlist.track_paths.len().to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_playlist_tab(ix);
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                this.add_track_to_playlist(drag.track_ix, ix);
                cx.notify();
            }))
    }

    pub(super) fn render_nav_item(
        &self,
        label: &'static str,
        count: impl Into<SharedString>,
        active: bool,
        target: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bg = if active { 0x282a30 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0xb6b8bf };

        div()
            .id(SharedString::from(format!("nav-{label}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .active(|this| this.opacity(0.82))
            .child(label)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(count.into()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if target == Page::Library {
                    this.open_all_music_tab();
                } else {
                    this.open_page(target);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn render_queue(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.right_sidebar_collapsed;

        if collapsed || self.queue.is_empty() {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        div()
            .w(px(RIGHT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_l_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17161b))
            .child(
                div()
                    .w(px(RIGHT_SIDEBAR_W))
                    .h(px(54.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(0x24252b))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Self::sidebar_button("›", "toggle-right-sidebar").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.right_sidebar_collapsed = !this.right_sidebar_collapsed;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(0xf0f0f4))
                                    .child("Up Next"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x676b74))
                            .child(format!("{} tracks", self.queue.len())),
                    ),
            )
            .child(
                div().w(px(RIGHT_SIDEBAR_W)).children(
                    self.queue
                        .iter()
                        .filter(|track_ix| **track_ix < self.tracks.len())
                        .enumerate()
                        .map(|(ix, track_ix)| self.render_queue_row(ix, &self.tracks[*track_ix])),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_queue_row(&self, ix: usize, track: &Track) -> impl IntoElement {
        let active = ix == 0;
        let bg = if active { 0x242329 } else { 0x17161b };

        div()
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(bg))
            .child(
                div()
                    .w(px(22.0))
                    .text_xs()
                    .text_color(rgb(0x70747d))
                    .child(format!("{}", ix + 1)),
            )
            .child(Self::album_tile(track, 28.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(if active { 0xeeb17d } else { 0xe2e2e7 }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x878b94))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(track.duration.clone()),
            )
    }
}
