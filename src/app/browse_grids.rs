use super::*;

impl TempoApp {
    pub(super) fn render_artists_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let subtitle = format!(
            "{} artists  ·  {} local albums",
            self.artists.len(),
            self.albums.len()
        );

        div()
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_grid_header("Artists", subtitle, cx))
            .child(
                div()
                    .id("artists-grid-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .when(self.artists.is_empty(), |this| {
                        this.child(self.render_empty_grid_message(
                            "No artists yet",
                            "Add a music folder and Tempo will group indexed tracks by artist.",
                        ))
                    })
                    .children(
                        self.artists
                            .iter()
                            .map(|artist| self.render_artist_card(artist, cx)),
                    ),
            )
    }

    pub(super) fn render_albums_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let subtitle = format!(
            "{} albums  ·  {} tracks",
            self.albums.len(),
            self.tracks.len()
        );

        div()
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_grid_header("Albums", subtitle, cx))
            .child(
                div()
                    .id("albums-grid-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .when(self.albums.is_empty(), |this| {
                        this.child(self.render_empty_grid_message(
                            "No albums yet",
                            "Add a music folder and Tempo will group indexed tracks by album.",
                        ))
                    })
                    .children(
                        self.albums
                            .iter()
                            .map(|album| self.render_album_card(album, cx)),
                    ),
            )
    }

    fn render_browse_grid_header(
        &self,
        title: &'static str,
        subtitle: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .h(px(54.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
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
                            .child(title),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(subtitle),
            )
            .child(div().flex_1())
            .child(
                self.sidebar_button("⚙", "open-settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_page(Page::Settings);
                        cx.notify();
                    })),
            )
    }

    fn render_artist_card(&self, artist: &Artist, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let name = artist.name.clone();

        div()
            .id(SharedString::from(format!(
                "artist-card-{}",
                artist.artist_id
            )))
            .w(px(154.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel_alt))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |this| {
                this.bg(rgb(colors.hover))
                    .border_color(rgb(colors.border_strong))
            })
            .active(|this| this.opacity(0.88))
            .child(self.square_grid_image(
                artist.photo_path.as_ref(),
                artist.initials.clone(),
                artist.color,
                154.0,
            ))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(artist.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(format!(
                                "{} albums  ·  {} tracks",
                                artist.album_count, artist.track_count
                            )),
                    )
                    .when_some(artist.bio.as_ref(), |this, bio| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.text_faint))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(bio.clone()),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_all_music_tab();
                this.set_search_query(name.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_album_card(&self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let search = format!("{} {}", album.artist, album.title);

        div()
            .id(SharedString::from(format!(
                "album-card-{}-{}",
                album.artist_id, album.album_id
            )))
            .w(px(154.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel_alt))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |this| {
                this.bg(rgb(colors.hover))
                    .border_color(rgb(colors.border_strong))
            })
            .active(|this| this.opacity(0.88))
            .child(self.square_grid_image(
                album.artwork_path.as_ref(),
                album.initials.clone(),
                album.color,
                154.0,
            ))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(album.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(album.artist.clone()),
                    )
                    .child(
                        div().text_xs().text_color(rgb(colors.text_faint)).child(
                            album
                                .year
                                .as_ref()
                                .map(|year| format!("{}  ·  {} tracks", year, album.track_count))
                                .unwrap_or_else(|| format!("{} tracks", album.track_count)),
                        ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_all_music_tab();
                this.set_search_query(search.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn square_grid_image(
        &self,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
        size: f32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .w(px(size))
            .h(px(size))
            .border_b_1()
            .border_color(rgb(colors.border))
            .overflow_hidden()
            .child(match path {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => Self::album_tile_fallback(initials, color, colors),
            })
            .into_any_element()
    }

    fn render_empty_grid_message(&self, title: &'static str, body: &'static str) -> AnyElement {
        let colors = *self.colors();

        div()
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .p_5()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child(title),
            )
            .child(div().text_color(rgb(colors.text_muted)).child(body))
            .into_any_element()
    }
}
