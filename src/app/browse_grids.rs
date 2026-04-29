use super::*;

#[derive(Clone, Copy)]
struct BrowseTableColumn {
    title: &'static str,
    width: Option<f32>,
}

impl TempoApp {
    pub(super) fn render_detail_hero(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.active_tab().source {
            TabSource::Artist(artist_id) => self.render_artist_detail_hero(artist_id, cx),
            TabSource::Album(album_id) => self.render_album_detail_hero(album_id, cx),
            TabSource::Library | TabSource::Playlist(_) => None,
        }
    }

    fn render_artist_detail_hero(
        &self,
        artist_id: i64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let artist = self.artist_by_id(artist_id)?;
        let colors = *self.colors();
        let albums = self.albums_for_artist(artist.artist_id);

        Some(
            div()
                .id(SharedString::from(format!("artist-hero-{artist_id}")))
                .flex_none()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.elevated))
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .items_center()
                        .child(self.hero_image(
                            SharedString::from(format!("artist-hero-image-{artist_id}")),
                            artist.photo_path.as_ref(),
                            artist.initials.clone(),
                            artist.color,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors.accent))
                                        .child("ARTIST"),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(colors.text_strong))
                                        .child(artist.name.clone()),
                                )
                                .child(div().text_color(rgb(colors.text_muted)).child(format!(
                                    "{} albums  ·  {} tracks",
                                    artist.album_count, artist.track_count
                                )))
                                .child(div().text_color(rgb(colors.text)).child(
                                    artist.bio.clone().unwrap_or_else(|| {
                                        format!(
                                            "{} is represented by {} local albums in your library.",
                                            artist.name, artist.album_count
                                        )
                                    }),
                                )),
                        ),
                )
                .when(!albums.is_empty(), |this| {
                    this.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_faint))
                                    .child("ALBUMS"),
                            )
                            .child(self.render_artist_album_grid(&albums, cx)),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_album_detail_hero(
        &self,
        album_id: i64,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let album = self.album_by_id(album_id)?;
        let colors = *self.colors();
        let artist_bio = self
            .artist_by_id(album.artist_id)
            .and_then(|artist| artist.bio.clone());
        let description = artist_bio.unwrap_or_else(|| {
            album
                .year
                .as_ref()
                .map(|year| {
                    format!(
                        "A {year} local album by {}, collected in your library with {} tracks.",
                        album.artist, album.track_count
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "A local album by {}, collected in your library with {} tracks.",
                        album.artist, album.track_count
                    )
                })
        });

        Some(
            div()
                .id(SharedString::from(format!("album-hero-{album_id}")))
                .flex_none()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.elevated))
                .flex()
                .gap_4()
                .items_center()
                .child(self.hero_image(
                    SharedString::from(format!("album-hero-image-{album_id}")),
                    album.artwork_path.as_ref(),
                    album.initials.clone(),
                    album.color,
                ))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.accent))
                                .child("ALBUM"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors.text_strong))
                                .child(album.title.clone()),
                        )
                        .child(div().text_color(rgb(colors.text_muted)).child(format!(
                                "{}  ·  {}  ·  {} tracks",
                                album.artist,
                                album.year.clone().unwrap_or_else(|| "Unknown year".to_string()),
                                album.track_count
                            )))
                        .child(div().text_color(rgb(colors.text)).child(description)),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_artists_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let subtitle = format!(
            "{} artists  ·  {} local albums",
            self.artists.len(),
            self.albums.len()
        );
        let grid_columns = self.browse_grid_columns(window);

        div()
            .id("artists-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header("Artists", subtitle, self.artist_view_mode, cx))
            .child(match self.artist_view_mode {
                BrowseViewMode::Grid => self.render_artist_grid(grid_columns, cx),
                BrowseViewMode::Table => self.render_artist_table(cx),
            })
    }

    pub(super) fn render_albums_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let subtitle = format!(
            "{} albums  ·  {} tracks",
            self.albums.len(),
            self.tracks.len()
        );
        let grid_columns = self.browse_grid_columns(window);

        div()
            .id("albums-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header("Albums", subtitle, self.album_view_mode, cx))
            .child(match self.album_view_mode {
                BrowseViewMode::Grid => self.render_album_grid(grid_columns, cx),
                BrowseViewMode::Table => self.render_album_table(cx),
            })
    }

    fn render_artist_grid(&self, columns: usize, cx: &mut Context<Self>) -> AnyElement {
        self.render_browse_grid(
            "artists-grid-scroll",
            "artist-grid-rows",
            "No artists yet",
            "Add a music folder and Tempo will group indexed tracks by artist.",
            self.artists.len(),
            columns,
            Self::render_artist_grid_row,
            cx,
        )
    }

    fn render_album_grid(&self, columns: usize, cx: &mut Context<Self>) -> AnyElement {
        self.render_browse_grid(
            "albums-grid-scroll",
            "album-grid-rows",
            "No albums yet",
            "Add a music folder and Tempo will group indexed tracks by album.",
            self.albums.len(),
            columns,
            Self::render_album_grid_row,
            cx,
        )
    }

    fn browse_grid_columns(&self, window: &Window) -> usize {
        let sidebar_width = if self.left_sidebar_collapsed {
            0.0
        } else {
            LEFT_SIDEBAR_W
        };
        let width = f32::from(window.viewport_size().width);
        let available = (width - sidebar_width - BROWSE_GRID_PAD_X).max(BROWSE_GRID_CARD_W);
        ((available + BROWSE_GRID_GAP) / (BROWSE_GRID_CARD_W + BROWSE_GRID_GAP))
            .floor()
            .max(1.0) as usize
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_grid(
        &self,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        item_count: usize,
        columns: usize,
        render_row: fn(&Self, usize, usize, &mut Context<Self>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item_count == 0 {
            return div()
                .id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .child(self.render_empty_grid_message(empty_title, empty_body))
                .into_any_element();
        }

        let row_count = item_count.div_ceil(columns);
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .p_4()
            .child(
                uniform_list(
                    list_id,
                    row_count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|row_ix| render_row(this, row_ix, columns, cx))
                            .collect()
                    }),
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn render_artist_grid_row(
        &self,
        row_ix: usize,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let start = row_ix * columns;
        let end = (start + columns).min(self.artists.len());

        div()
            .id(SharedString::from(format!("artist-grid-row-{row_ix}")))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                self.artists[start..end]
                    .iter()
                    .map(|artist| self.render_artist_card(artist, cx)),
            )
            .into_any_element()
    }

    fn render_album_grid_row(
        &self,
        row_ix: usize,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let start = row_ix * columns;
        let end = (start + columns).min(self.albums.len());

        div()
            .id(SharedString::from(format!("album-grid-row-{row_ix}")))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                self.albums[start..end]
                    .iter()
                    .map(|album| self.render_album_card(album, cx)),
            )
            .into_any_element()
    }

    fn render_artist_table(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_browse_table(
            "artists-table",
            "artist-table-rows",
            "No artists yet",
            "Add a music folder and Tempo will group indexed tracks by artist.",
            self.artists.len(),
            &[
                BrowseTableColumn {
                    title: "",
                    width: Some(42.0),
                },
                BrowseTableColumn {
                    title: "Artist",
                    width: None,
                },
                BrowseTableColumn {
                    title: "Albums",
                    width: Some(92.0),
                },
                BrowseTableColumn {
                    title: "Tracks",
                    width: Some(92.0),
                },
            ],
            Self::render_artist_row,
            cx,
        )
    }

    fn render_album_table(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_browse_table(
            "albums-table",
            "album-table-rows",
            "No albums yet",
            "Add a music folder and Tempo will group indexed tracks by album.",
            self.albums.len(),
            &[
                BrowseTableColumn {
                    title: "",
                    width: Some(42.0),
                },
                BrowseTableColumn {
                    title: "Album",
                    width: None,
                },
                BrowseTableColumn {
                    title: "Artist",
                    width: Some(220.0),
                },
                BrowseTableColumn {
                    title: "Year",
                    width: Some(90.0),
                },
                BrowseTableColumn {
                    title: "Tracks",
                    width: Some(92.0),
                },
            ],
            Self::render_album_row,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_table(
        &self,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        row_count: usize,
        columns: &'static [BrowseTableColumn],
        render_row: fn(&Self, usize, &mut Context<Self>) -> Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(colors.border))
            .child(self.render_browse_table_header(columns))
            .when(row_count == 0, |this| {
                this.child(
                    div()
                        .p_4()
                        .child(self.render_empty_grid_message(empty_title, empty_body)),
                )
            })
            .when(row_count > 0, |this| {
                this.child(
                    uniform_list(
                        list_id,
                        row_count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            range
                                .filter_map(|row_ix| render_row(this, row_ix, cx))
                                .collect()
                        }),
                    )
                    .flex_1()
                    .min_h_0(),
                )
            })
            .into_any_element()
    }

    fn render_browse_header(
        &self,
        title: &'static str,
        subtitle: String,
        mode: BrowseViewMode,
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
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        self.render_view_mode_button("Grid", title, mode == BrowseViewMode::Grid)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_browse_view_mode(title, BrowseViewMode::Grid);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_view_mode_button("Table", title, mode == BrowseViewMode::Table)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_browse_view_mode(title, BrowseViewMode::Table);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                self.sidebar_button("⚙", "open-settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_page(Page::Settings);
                        cx.notify();
                    })),
            )
    }

    fn render_view_mode_button(
        &self,
        label: &'static str,
        page: &'static str,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.button
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let border = if active {
            colors.border_strong
        } else {
            colors.waveform_border
        };

        div()
            .id(SharedString::from(format!(
                "{}-{}-view",
                page.to_ascii_lowercase(),
                label.to_ascii_lowercase()
            )))
            .h(px(24.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(fg))
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .text_color(rgb(colors.text_strong))
            })
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    fn set_browse_view_mode(&mut self, page: &'static str, mode: BrowseViewMode) {
        match page {
            "Artists" => self.artist_view_mode = mode,
            "Albums" => self.album_view_mode = mode,
            _ => {}
        }
    }

    fn render_browse_table_header(&self, columns: &[BrowseTableColumn]) -> AnyElement {
        let colors = *self.colors();

        div()
            .h(px(34.0))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_xs()
            .text_color(rgb(colors.text_faint))
            .children(
                columns
                    .iter()
                    .map(|column| Self::render_browse_table_cell(column.width, column.title)),
            )
            .into_any_element()
    }

    fn render_artist_row(&self, row_ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let artist = self.artists.get(row_ix)?;
        let colors = *self.colors();
        let bg = if row_ix.is_multiple_of(2) {
            colors.surface
        } else {
            colors.panel_alt
        };
        let artist_id = artist.artist_id;

        div()
            .id(SharedString::from(format!(
                "artist-row-{}",
                artist.artist_id
            )))
            .h(px(50.0))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .child(self.row_image(
                SharedString::from(format!("artist-row-image-{}", artist.artist_id)),
                artist.photo_path.as_ref(),
                artist.initials.clone(),
                artist.color,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(artist.name.clone()),
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
            .child(
                div()
                    .w(px(92.0))
                    .text_color(rgb(colors.text_muted))
                    .child(artist.album_count.to_string()),
            )
            .child(
                div()
                    .w(px(92.0))
                    .text_color(rgb(colors.text_muted))
                    .child(artist.track_count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_artist_tab(artist_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_album_row(&self, row_ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let album = self.albums.get(row_ix)?;
        let colors = *self.colors();
        let bg = if row_ix.is_multiple_of(2) {
            colors.surface
        } else {
            colors.panel_alt
        };
        let album_id = album.album_id;

        div()
            .id(SharedString::from(format!(
                "album-row-{}-{}",
                album.artist_id, album.album_id
            )))
            .h(px(50.0))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .child(self.row_image(
                SharedString::from(format!(
                    "album-row-image-{}-{}",
                    album.artist_id, album.album_id
                )),
                album.artwork_path.as_ref(),
                album.initials.clone(),
                album.color,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(album.title.clone()),
            )
            .child(
                div()
                    .w(px(220.0))
                    .min_w_0()
                    .text_color(rgb(colors.text_muted))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(album.artist.clone()),
            )
            .child(
                div()
                    .w(px(90.0))
                    .text_color(rgb(colors.text_muted))
                    .child(album.year.clone().unwrap_or_else(|| "Unknown".to_string())),
            )
            .child(
                div()
                    .w(px(92.0))
                    .text_color(rgb(colors.text_muted))
                    .child(album.track_count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_album_tab(album_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_browse_table_cell(width: Option<f32>, child: impl IntoElement) -> AnyElement {
        let cell = div().min_w_0().overflow_hidden().text_ellipsis();
        match width {
            Some(width) => cell.w(px(width)).child(child).into_any_element(),
            None => cell.flex_1().child(child).into_any_element(),
        }
    }

    fn render_artist_card(&self, artist: &Artist, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let artist_id = artist.artist_id;

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
                SharedString::from(format!("artist-card-image-{}", artist.artist_id)),
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
                this.open_artist_tab(artist_id);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_album_card(&self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let album_id = album.album_id;

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
                SharedString::from(format!(
                    "album-card-image-{}-{}",
                    album.artist_id, album.album_id
                )),
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
                this.open_album_tab(album_id);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_artist_album_grid(&self, albums: &[&Album], cx: &mut Context<Self>) -> AnyElement {
        let rows = albums
            .chunks(4)
            .enumerate()
            .map(|(row_ix, row)| {
                div()
                    .id(SharedString::from(format!(
                        "artist-album-hero-row-{row_ix}"
                    )))
                    .flex()
                    .gap_3()
                    .children(row.iter().map(|album| self.render_album_card(album, cx)))
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(rows)
            .into_any_element()
    }

    fn hero_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
            .w(px(132.0))
            .h(px(132.0))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .overflow_hidden()
            .shadow_lg()
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

    fn square_grid_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
        size: f32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
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

    fn row_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
            .w(px(38.0))
            .h(px(38.0))
            .flex_none()
            .rounded_sm()
            .border_1()
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
