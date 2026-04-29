use super::*;

impl TempoApp {
    pub(super) fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.left_sidebar_collapsed;
        let colors = *self.colors();

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
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
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
                            .border_color(rgb(colors.border))
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .flex()
                            .justify_between()
                            .child(format!("{} tracks", self.tracks.len()))
                            .child(Self::format_library_size(&self.tracks)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .h(px(50.0))
            .flex()
            .items_center()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child("Tempo"),
            )
            .child(
                self.sidebar_button("‹", "toggle-left-sidebar")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_sidebar_collapsed = !this.left_sidebar_collapsed;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn sidebar_button(
        &self,
        label: &'static str,
        id: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();

        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .bg(rgb(colors.button))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(colors.text_muted))
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .text_color(rgb(colors.text_strong))
            })
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
            .child(self.nav_group_title("LIBRARY"))
            .child(self.render_nav_item(
                "All Music",
                self.tracks.len().to_string(),
                self.page == Page::Library && self.active_tab().source == TabSource::Library,
                Page::Library,
                cx,
            ))
            .child(self.render_nav_item(
                "Artists",
                self.artists.len().to_string(),
                self.page == Page::Artists,
                Page::Artists,
                cx,
            ))
            .child(self.render_nav_item(
                "Albums",
                self.albums.len().to_string(),
                self.page == Page::Albums,
                Page::Albums,
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
                    .child(self.nav_group_title("PLAYLISTS"))
                    .child(
                        self.sidebar_button("+", "new-playlist")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            })),
                    ),
            )
            .when(self.playlists.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(self.colors().text_faint))
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

    pub(super) fn nav_group_title(&self, title: &'static str) -> impl IntoElement {
        div()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(self.colors().text_faint))
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
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.panel
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text
        };

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
                    .text_color(rgb(colors.text_faint))
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
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.panel
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text
        };

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
                    .text_color(rgb(colors.text_faint))
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
        let colors = *self.colors();

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
            .border_color(rgb(colors.border))
            .bg(rgb(colors.queue))
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
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.sidebar_button("›", "toggle-right-sidebar").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.right_sidebar_collapsed = !this.right_sidebar_collapsed;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_strong))
                                    .child("Up Next"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
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
        let colors = *self.colors();
        let bg = if active {
            colors.queue_active
        } else {
            colors.queue
        };

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
                    .text_color(rgb(colors.text_faint))
                    .child(format!("{}", ix + 1)),
            )
            .child(self.album_tile(track, 28.0))
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
                            .text_color(rgb(if active {
                                colors.accent
                            } else {
                                colors.text_strong
                            }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(track.duration.clone()),
            )
    }
}
