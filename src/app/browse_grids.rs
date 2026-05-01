use super::*;

#[derive(Clone, Copy)]
struct BrowseGridMetrics {
    columns: usize,
    card_width: f32,
}

impl TempoApp {
    pub(super) fn render_detail_hero(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.active_tab().source {
            TabSource::Artist(artist_id) => self.render_artist_detail_hero(*artist_id, cx),
            TabSource::Album(album_id) => self.render_album_detail_hero(*album_id, cx),
            TabSource::Genre(genre_key) => self.render_genre_detail_hero(genre_key, cx),
            TabSource::Library | TabSource::Playlist(_) => None,
        }
    }

    fn render_artist_detail_hero(
        &self,
        artist_id: i64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let artist = self.artist_by_id(artist_id)?;
        if artist.photo_path.is_none() || artist.bio.is_none() {
            self.queue_artist_metadata_demand(artist.artist_id);
        }
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
                            .child(self.render_artist_album_strip(&albums, cx)),
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
        if album.artwork_path.is_none() {
            self.queue_album_cover_demand(album.album_id);
        }
        // Trigger an on-demand description fetch the first time this
        // hero is rendered without a description already in hand. The
        // demand queue dedupes per-session so this is cheap. Skipped
        // when the album is in the `Unavailable` terminal state.
        if album.description.is_none()
            && album.description_state != AlbumDescriptionState::Unavailable
        {
            self.queue_album_profile_demand(album.album_id);
        }
        let colors = *self.colors();
        // Description priority:
        //   1. Real online description from TheAudioDB / Wikipedia /
        //      Discogs (Phase 1 fills in #1; Phase 2/3 add the others).
        //   2. Synthetic local fallback while still pending.
        //   3. "No online description available." once every fallback
        //      source has been exhausted.
        let description = match (album.description.clone(), album.description_state) {
            (Some(text), _) if !text.trim().is_empty() => text,
            (_, AlbumDescriptionState::Unavailable) => {
                "No online description available.".to_string()
            }
            _ => album
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
                }),
        };

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

    fn render_genre_detail_hero(
        &self,
        genre_key: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let genre = self.genre_by_key(genre_key)?;
        let colors = *self.colors();
        let play_key = genre.key.clone();
        let shuffle_key = genre.key.clone();
        for album in &genre.top_albums {
            if let Some(album_id) = album.album_id
                && album.artwork_path.is_none()
            {
                self.queue_album_cover_demand(album_id);
            }
        }

        Some(
            div()
                .id(SharedString::from(format!("genre-hero-{genre_key}")))
                .flex_none()
                .border_b_1()
                .border_color(rgb(colors.border))
                .bg(rgb(blend_rgb(genre.color, colors.elevated, 0.74)))
                .flex()
                .flex_col()
                .gap_4()
                .px_4()
                .py_4()
                .child(
                    div()
                        .id("all-genres-link")
                        .text_xs()
                        .text_color(rgb(colors.text_muted))
                        .cursor_pointer()
                        .child("‹ All genres")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_page(Page::Genres);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap_5()
                        .items_center()
                        .child(self.render_genre_album_fan(
                            SharedString::from(format!("genre-hero-fan-{genre_key}")),
                            &genre.top_albums,
                            genre.color,
                            126.0,
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
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(colors.text_faint))
                                        .child("GENRE"),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(colors.text_strong))
                                        .child(genre.name.clone()),
                                )
                                .child(div().text_color(rgb(colors.text_muted)).child(format!(
                                    "{} artists  ·  {} albums  ·  {} tracks  ·  {}",
                                    genre.artist_count,
                                    genre.album_count,
                                    genre.track_count,
                                    format_duration_compact(genre.duration_value)
                                )))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(self.genre_action_button("Play all", true).on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.play_genre(&play_key, false, cx);
                                                cx.notify();
                                            }),
                                        ))
                                        .child(
                                            self.genre_action_button("Shuffle", false).on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.play_genre(&shuffle_key, true, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .when(!genre.artists.is_empty(), |this| {
                    this.child(self.render_genre_artist_pills(genre))
                })
                .when(!genre.albums.is_empty(), |this| {
                    this.child(self.render_genre_album_strip(genre, cx))
                })
                .into_any_element(),
        )
    }

    pub(super) fn render_artists_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let mut artist_indices = self.artist_indices_for_search_query(&self.browse_search_query);
        if self.browse_view_mode() == BrowseViewMode::Table {
            self.sort_artist_indices(&mut artist_indices);
        }
        let is_searching = !self.browse_search_query.trim().is_empty();
        let subtitle = if is_searching {
            format!(
                "{} of {} artists  ·  {} local albums",
                artist_indices.len(),
                self.artists.len(),
                self.albums.len()
            )
        } else {
            format!(
                "{} artists  ·  {} local albums",
                self.artists.len(),
                self.albums.len()
            )
        };
        let grid = self.browse_grid_metrics(window);

        div()
            .id("artists-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header(window, "Artists", subtitle, cx))
            .child(
                self.render_tab_bar_with_controls(Some(("Artists", self.browse_view_mode())), cx),
            )
            .child(match self.browse_view_mode() {
                BrowseViewMode::Grid => self.render_artist_grid(grid, artist_indices, cx),
                BrowseViewMode::Table => self.render_artist_table(artist_indices, cx),
            })
    }

    pub(super) fn render_albums_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let mut album_indices = self.album_indices_for_search_query(&self.browse_search_query);
        if self.browse_view_mode() == BrowseViewMode::Table {
            self.sort_album_indices(&mut album_indices);
        }
        let is_searching = !self.browse_search_query.trim().is_empty();
        let subtitle = if is_searching {
            format!(
                "{} of {} albums  ·  {} tracks",
                album_indices.len(),
                self.albums.len(),
                self.tracks.len()
            )
        } else {
            format!(
                "{} albums  ·  {} tracks",
                self.albums.len(),
                self.tracks.len()
            )
        };
        let grid = self.browse_grid_metrics(window);

        div()
            .id("albums-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header(window, "Albums", subtitle, cx))
            .child(self.render_tab_bar_with_controls(Some(("Albums", self.browse_view_mode())), cx))
            .child(match self.browse_view_mode() {
                BrowseViewMode::Grid => self.render_album_grid(grid, album_indices, cx),
                BrowseViewMode::Table => self.render_album_table(album_indices, cx),
            })
    }

    pub(super) fn render_genres_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let mut genre_indices = self.genre_indices_for_search_query(&self.browse_search_query);
        if self.browse_view_mode() == BrowseViewMode::Table {
            self.sort_genre_indices(&mut genre_indices);
        }
        let is_searching = !self.browse_search_query.trim().is_empty();
        let subtitle = if is_searching {
            format!(
                "{} of {} genres  ·  {} tracks",
                genre_indices.len(),
                self.genres.len(),
                self.tracks.len()
            )
        } else {
            format!(
                "{} genres  ·  {} tracks",
                self.genres.len(),
                self.tracks.len()
            )
        };
        let grid = self.genre_grid_metrics(window);

        div()
            .id("genres-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_genres_header(window, subtitle, cx))
            .child(self.render_tab_bar_with_controls(Some(("Genres", self.browse_view_mode())), cx))
            .child(match self.browse_view_mode() {
                BrowseViewMode::Grid => self.render_genre_grid(grid, genre_indices, cx),
                BrowseViewMode::Table => self.render_genre_table(genre_indices, cx),
            })
    }

    fn render_genres_header(
        &self,
        window: &Window,
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
                            .child("Genres"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(subtitle),
            )
            .when_some(self.render_metadata_status(cx), |this, status| {
                this.child(status)
            })
            .child(div().flex_1())
            .child(self.render_search_box(window, "Search", cx))
            .child(self.with_tooltip(
                self.render_eq_header_button("genres-open-eq", cx),
                "genres-open-eq-tooltip",
                "Equalizer (right-click to toggle)",
                cx,
            ))
            .child(
                self.with_tooltip(
                    self.sidebar_button("⚙", "genres-open-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_page(Page::Settings);
                            cx.notify();
                        })),
                    "genres-open-settings-tooltip",
                    "Settings",
                    cx,
                ),
            )
    }

    fn render_artist_grid(
        &self,
        grid: BrowseGridMetrics,
        artist_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_grid(
            BrowseScrollbarTarget::ArtistsGrid,
            "artists-grid-scroll",
            "artist-grid-rows",
            if is_searching {
                "No matching artists"
            } else {
                "No artists yet"
            },
            if is_searching {
                "No artists match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by artist."
            },
            artist_indices,
            grid,
            Self::render_artist_grid_row,
            cx,
        )
    }

    fn render_album_grid(
        &self,
        grid: BrowseGridMetrics,
        album_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_grid(
            BrowseScrollbarTarget::AlbumsGrid,
            "albums-grid-scroll",
            "album-grid-rows",
            if is_searching {
                "No matching albums"
            } else {
                "No albums yet"
            },
            if is_searching {
                "No albums match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by album."
            },
            album_indices,
            grid,
            Self::render_album_grid_row,
            cx,
        )
    }

    fn render_genre_grid(
        &self,
        grid: BrowseGridMetrics,
        genre_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_grid(
            BrowseScrollbarTarget::GenresGrid,
            "genres-grid-scroll",
            "genre-grid-rows",
            if is_searching {
                "No matching genres"
            } else {
                "No genres yet"
            },
            if is_searching {
                "No genres match the current search."
            } else {
                "Add genre tags to your music and Tempo will group them here."
            },
            genre_indices,
            grid,
            Self::render_genre_grid_row,
            cx,
        )
    }

    fn browse_grid_metrics(&self, window: &Window) -> BrowseGridMetrics {
        let sidebar_width = if self.left_sidebar_collapsed {
            0.0
        } else {
            LEFT_SIDEBAR_W
        };
        let width = f32::from(window.viewport_size().width);
        let available = (width - sidebar_width - BROWSE_GRID_PAD_X).max(BROWSE_GRID_CARD_W);
        let columns = ((available + BROWSE_GRID_GAP) / (BROWSE_GRID_CARD_W + BROWSE_GRID_GAP))
            .floor()
            .max(1.0) as usize;
        let total_gap = BROWSE_GRID_GAP * columns.saturating_sub(1) as f32;
        let card_width = ((available - total_gap) / columns as f32).max(BROWSE_GRID_CARD_W);

        BrowseGridMetrics {
            columns,
            card_width,
        }
    }

    fn genre_grid_metrics(&self, window: &Window) -> BrowseGridMetrics {
        let sidebar_width = if self.left_sidebar_collapsed {
            0.0
        } else {
            LEFT_SIDEBAR_W
        };
        let width = f32::from(window.viewport_size().width);
        let available = (width - sidebar_width - BROWSE_GRID_PAD_X).max(GENRE_GRID_CARD_W);
        let columns = ((available + BROWSE_GRID_GAP) / (GENRE_GRID_CARD_W + BROWSE_GRID_GAP))
            .floor()
            .max(1.0) as usize;
        let total_gap = BROWSE_GRID_GAP * columns.saturating_sub(1) as f32;
        let card_width = ((available - total_gap) / columns as f32).max(GENRE_GRID_CARD_W);

        BrowseGridMetrics {
            columns,
            card_width,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_grid(
        &self,
        target: BrowseScrollbarTarget,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        item_indices: Vec<usize>,
        grid: BrowseGridMetrics,
        render_row: fn(&Self, usize, BrowseGridMetrics, &[usize], &mut Context<Self>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item_count = item_indices.len();
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

        let item_indices = Arc::new(item_indices);
        let columns = grid.columns;
        let row_count = item_count.div_ceil(columns);
        let scroll_handle = self.browse_scroll_handle(target);
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .p_4()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.drag_browse_scrollbar(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                uniform_list(
                    list_id,
                    row_count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        let visible = range.end.saturating_sub(range.start);
                        let _build_span = perf::span(
                            "browse.uniform_list.build",
                            format!(
                                "target={} rows={} range={}..{} columns={}",
                                Self::browse_scrollbar_target_id(target),
                                visible,
                                range.start,
                                range.end,
                                columns
                            ),
                        );
                        let item_indices = item_indices.clone();
                        range
                            .map(|row_ix| render_row(this, row_ix, grid, &item_indices, cx))
                            .collect()
                    }),
                )
                .size_full()
                .track_scroll(scroll_handle),
            )
            .child(self.render_browse_scrollbar(target, row_count, cx))
            .into_any_element()
    }

    fn render_artist_grid_row(
        &self,
        row_ix: usize,
        grid: BrowseGridMetrics,
        artist_indices: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = grid.columns;
        let start = row_ix * columns;
        let end = (start + columns).min(artist_indices.len());

        div()
            // Per-row id: `NamedInteger` avoids the per-frame String
            // allocation that the previous `format!()` produced for
            // each visible row.
            .id(gpui::ElementId::NamedInteger(
                "artist-grid-row".into(),
                row_ix as u64,
            ))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                artist_indices[start..end]
                    .iter()
                    .filter_map(|artist_ix| self.artists.get(*artist_ix))
                    .map(|artist| self.render_artist_card(artist, grid.card_width, cx)),
            )
            .into_any_element()
    }

    fn render_album_grid_row(
        &self,
        row_ix: usize,
        grid: BrowseGridMetrics,
        album_indices: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = grid.columns;
        let start = row_ix * columns;
        let end = (start + columns).min(album_indices.len());

        div()
            .id(gpui::ElementId::NamedInteger(
                "album-grid-row".into(),
                row_ix as u64,
            ))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                album_indices[start..end]
                    .iter()
                    .filter_map(|album_ix| self.albums.get(*album_ix))
                    .map(|album| self.render_album_card(album, grid.card_width, cx)),
            )
            .into_any_element()
    }

    fn render_genre_grid_row(
        &self,
        row_ix: usize,
        grid: BrowseGridMetrics,
        genre_indices: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = grid.columns;
        let start = row_ix * columns;
        let end = (start + columns).min(genre_indices.len());

        div()
            .id(gpui::ElementId::NamedInteger(
                "genre-grid-row".into(),
                row_ix as u64,
            ))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                genre_indices[start..end]
                    .iter()
                    .filter_map(|genre_ix| self.genres.get(*genre_ix))
                    .map(|genre| self.render_genre_card(genre, grid.card_width, cx)),
            )
            .into_any_element()
    }

    fn render_artist_table(
        &self,
        artist_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_table(
            BrowseScrollbarTarget::ArtistsTable,
            "artists-table",
            "artist-table-rows",
            if is_searching {
                "No matching artists"
            } else {
                "No artists yet"
            },
            if is_searching {
                "No artists match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by artist."
            },
            artist_indices,
            ColumnMenuKind::Artists,
            self.visible_artist_columns
                .iter()
                .copied()
                .map(ColumnResizeTarget::Artist)
                .collect(),
            Self::render_artist_row,
            cx,
        )
    }

    fn render_album_table(&self, album_indices: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_table(
            BrowseScrollbarTarget::AlbumsTable,
            "albums-table",
            "album-table-rows",
            if is_searching {
                "No matching albums"
            } else {
                "No albums yet"
            },
            if is_searching {
                "No albums match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by album."
            },
            album_indices,
            ColumnMenuKind::Albums,
            self.visible_album_columns
                .iter()
                .copied()
                .map(ColumnResizeTarget::Album)
                .collect(),
            Self::render_album_row,
            cx,
        )
    }

    fn render_genre_table(&self, genre_indices: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_table(
            BrowseScrollbarTarget::GenresGrid,
            "genres-table",
            "genre-table-rows",
            if is_searching {
                "No matching genres"
            } else {
                "No genres yet"
            },
            if is_searching {
                "No genres match the current search."
            } else {
                "Add genre tags to your music and Tempo will group them here."
            },
            genre_indices,
            ColumnMenuKind::Genres,
            self.visible_genre_columns
                .iter()
                .copied()
                .map(ColumnResizeTarget::Genre)
                .collect(),
            Self::render_genre_row,
            cx,
        )
    }

    pub(super) fn sort_artist_indices(&self, artist_indices: &mut [usize]) {
        artist_indices.sort_by(|left, right| {
            let Some(left) = self.artists.get(*left) else {
                return std::cmp::Ordering::Equal;
            };
            let Some(right) = self.artists.get(*right) else {
                return std::cmp::Ordering::Equal;
            };
            let ordering = match self.artist_table_sort_column {
                ArtistTableColumn::Artwork | ArtistTableColumn::Artist => {
                    left.name.cmp(&right.name)
                }
                ArtistTableColumn::Albums => left
                    .album_count
                    .cmp(&right.album_count)
                    .then(left.name.cmp(&right.name)),
                ArtistTableColumn::Tracks => left
                    .track_count
                    .cmp(&right.track_count)
                    .then(left.name.cmp(&right.name)),
                ArtistTableColumn::Duration => self
                    .artist_total_duration(left.artist_id)
                    .cmp(&self.artist_total_duration(right.artist_id))
                    .then(left.name.cmp(&right.name)),
            };
            match self.artist_table_sort_direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });
    }

    pub(super) fn sort_album_indices(&self, album_indices: &mut [usize]) {
        album_indices.sort_by(|left, right| {
            let Some(left) = self.albums.get(*left) else {
                return std::cmp::Ordering::Equal;
            };
            let Some(right) = self.albums.get(*right) else {
                return std::cmp::Ordering::Equal;
            };
            let ordering = match self.album_table_sort_column {
                AlbumTableColumn::Artwork | AlbumTableColumn::Album => left
                    .title
                    .cmp(&right.title)
                    .then(left.artist.cmp(&right.artist)),
                AlbumTableColumn::Artist => left
                    .artist
                    .cmp(&right.artist)
                    .then(left.title.cmp(&right.title)),
                AlbumTableColumn::Year => left
                    .year
                    .cmp(&right.year)
                    .then(left.title.cmp(&right.title)),
                AlbumTableColumn::Tracks => left
                    .track_count
                    .cmp(&right.track_count)
                    .then(left.title.cmp(&right.title)),
                AlbumTableColumn::Duration => self
                    .album_total_duration(left.album_id)
                    .cmp(&self.album_total_duration(right.album_id))
                    .then(left.title.cmp(&right.title)),
            };
            match self.album_table_sort_direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });
    }

    pub(super) fn sort_genre_indices(&self, genre_indices: &mut [usize]) {
        genre_indices.sort_by(|left, right| {
            let Some(left) = self.genres.get(*left) else {
                return std::cmp::Ordering::Equal;
            };
            let Some(right) = self.genres.get(*right) else {
                return std::cmp::Ordering::Equal;
            };
            let ordering = match self.genre_table_sort_column {
                GenreTableColumn::Genre => left.name.cmp(&right.name),
                GenreTableColumn::Artists => left
                    .artist_count
                    .cmp(&right.artist_count)
                    .then(left.name.cmp(&right.name)),
                GenreTableColumn::Albums => left
                    .album_count
                    .cmp(&right.album_count)
                    .then(left.name.cmp(&right.name)),
                GenreTableColumn::Tracks => left
                    .track_count
                    .cmp(&right.track_count)
                    .then(left.name.cmp(&right.name)),
                GenreTableColumn::Duration => left
                    .duration_value
                    .cmp(&right.duration_value)
                    .then(left.name.cmp(&right.name)),
            };
            match self.genre_table_sort_direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_table(
        &self,
        target: BrowseScrollbarTarget,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        row_indices: Vec<usize>,
        column_menu_kind: ColumnMenuKind,
        columns: Vec<ColumnResizeTarget>,
        render_row: fn(&Self, usize, usize, &mut Context<Self>) -> Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let row_count = row_indices.len();
        let row_indices = Arc::new(row_indices);
        let scroll_handle = self.browse_scroll_handle(target);

        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(colors.border))
            // Mouse-move/up live on the outer container so column resizing
            // works while the cursor is over the header *or* the body, and
            // so a left-mouse-down anywhere in the table dismisses an open
            // column menu (matching the All Music table behavior).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if this.column_menu_open {
                        this.column_menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    let scrolled = this.drag_browse_scrollbar(target, event);
                    let resized = !scrolled && this.resize_column_from_mouse(event);
                    if scrolled || resized {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    let scrolled = this.finish_browse_scrollbar_drag();
                    let resized = this.finish_column_resize();
                    if scrolled || resized {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    let scrolled = this.finish_browse_scrollbar_drag();
                    let resized = this.finish_column_resize();
                    if scrolled || resized {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_browse_table_header(column_menu_kind, 27.0, &columns, cx))
            .when(row_count == 0, |this| {
                this.child(
                    div()
                        .p_4()
                        .child(self.render_empty_grid_message(empty_title, empty_body)),
                )
            })
            .when(row_count > 0, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .overflow_hidden()
                        .child(
                            uniform_list(
                                list_id,
                                row_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    let visible = range.end.saturating_sub(range.start);
                                    let _build_span = perf::span(
                                        "browse.uniform_list.build",
                                        format!(
                                            "target={} rows={} range={}..{} mode=table",
                                            Self::browse_scrollbar_target_id(target),
                                            visible,
                                            range.start,
                                            range.end
                                        ),
                                    );
                                    let row_indices = row_indices.clone();
                                    range
                                        .filter_map(|row_ix| {
                                            let item_ix = *row_indices.get(row_ix)?;
                                            render_row(this, row_ix, item_ix, cx)
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .track_scroll(scroll_handle),
                        )
                        .child(self.render_browse_scrollbar(target, row_count, cx)),
                )
            })
            .into_any_element()
    }

    pub(super) fn browse_scroll_handle(
        &self,
        target: BrowseScrollbarTarget,
    ) -> UniformListScrollHandle {
        match target {
            BrowseScrollbarTarget::ArtistsGrid => self.artist_grid_scroll_handle.clone(),
            BrowseScrollbarTarget::ArtistsTable => self.artist_table_scroll_handle.clone(),
            BrowseScrollbarTarget::AlbumsGrid => self.album_grid_scroll_handle.clone(),
            BrowseScrollbarTarget::AlbumsTable => self.album_table_scroll_handle.clone(),
            BrowseScrollbarTarget::GenresGrid => self.genre_grid_scroll_handle.clone(),
            BrowseScrollbarTarget::PlaybackHistory => self.playback_history_scroll_handle.clone(),
            BrowseScrollbarTarget::Liked => self.liked_scroll_handle.clone(),
        }
    }

    fn browse_scrollbar_base_handle(&self, target: BrowseScrollbarTarget) -> gpui::ScrollHandle {
        self.browse_scroll_handle(target)
            .0
            .borrow()
            .base_handle
            .clone()
    }

    fn browse_scrollbar_metrics(
        &self,
        target: BrowseScrollbarTarget,
    ) -> Option<TableScrollbarMetrics> {
        let handle = self.browse_scroll_handle(target);
        let (base_handle, measured) = {
            let state = handle.0.borrow();
            (state.base_handle.clone(), state.last_item_size.is_some())
        };

        if !measured {
            return None;
        }

        let bounds = base_handle.bounds();
        let viewport_height = f32::from(bounds.size.height);
        if viewport_height <= 0.0 {
            return None;
        }

        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        let content_height = viewport_height + max_scroll;
        let track_height = (viewport_height - TABLE_SCROLLBAR_MARGIN * 2.0).max(1.0);
        let thumb_height = if content_height <= 0.0 {
            track_height
        } else {
            ((viewport_height / content_height) * track_height)
                .max(TABLE_SCROLLBAR_MIN_THUMB_H)
                .min(track_height)
        };
        let thumb_travel = (track_height - thumb_height).max(0.0);
        let scroll_top = (-f32::from(base_handle.offset().y)).clamp(0.0, max_scroll);
        let thumb_top = if max_scroll > 0.0 && thumb_travel > 0.0 {
            (scroll_top / max_scroll) * thumb_travel
        } else {
            0.0
        };

        Some(TableScrollbarMetrics {
            track_top: f32::from(bounds.origin.y) + TABLE_SCROLLBAR_MARGIN,
            track_height,
            thumb_top,
            thumb_height,
            max_scroll,
            scroll_top,
        })
    }

    fn begin_browse_scrollbar_drag(
        &mut self,
        target: BrowseScrollbarTarget,
        event: &MouseDownEvent,
    ) -> bool {
        let Some(metrics) = self.browse_scrollbar_metrics(target) else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let local_y = f32::from(event.position.y) - metrics.track_top;
        let thumb_bottom = metrics.thumb_top + metrics.thumb_height;
        let thumb_offset = if (metrics.thumb_top..=thumb_bottom).contains(&local_y) {
            local_y - metrics.thumb_top
        } else {
            metrics.thumb_height / 2.0
        };
        let start_offset = self.browse_scrollbar_base_handle(target).offset();

        self.browse_scrollbar_drag = Some(BrowseScrollbarDrag {
            target,
            thumb_offset,
            start_offset,
        });
        self.scroll_browse_to_scrollbar_y(target, event.position.y, thumb_offset);
        true
    }

    fn drag_browse_scrollbar(
        &mut self,
        target: BrowseScrollbarTarget,
        event: &MouseMoveEvent,
    ) -> bool {
        let Some(drag) = self.browse_scrollbar_drag else {
            return false;
        };

        if drag.target != target {
            return false;
        }

        if !event.dragging() {
            self.browse_scrollbar_drag = None;
            return false;
        }

        self.scroll_browse_to_scrollbar_y(target, event.position.y, drag.thumb_offset)
    }

    pub(super) fn finish_browse_scrollbar_drag(&mut self) -> bool {
        self.browse_scrollbar_drag.take().is_some()
    }

    pub(super) fn cancel_browse_scrollbar_drag(&mut self) -> bool {
        let Some(drag) = self.browse_scrollbar_drag.take() else {
            return false;
        };

        self.browse_scrollbar_base_handle(drag.target)
            .set_offset(drag.start_offset);
        true
    }

    fn scroll_browse_to_scrollbar_y(
        &mut self,
        target: BrowseScrollbarTarget,
        mouse_y: Pixels,
        thumb_offset: f32,
    ) -> bool {
        let Some(metrics) = self.browse_scrollbar_metrics(target) else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let thumb_travel = (metrics.track_height - metrics.thumb_height).max(1.0);
        let thumb_top =
            (f32::from(mouse_y) - metrics.track_top - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_top / thumb_travel;
        self.scroll_browse_to_ratio(target, ratio)
    }

    fn scroll_browse_to_ratio(&mut self, target: BrowseScrollbarTarget, ratio: f32) -> bool {
        let base_handle = self.browse_scrollbar_base_handle(target);
        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        if max_scroll <= 0.0 {
            return false;
        }

        let target_y = px(-(ratio.clamp(0.0, 1.0) * max_scroll));
        let current = base_handle.offset();
        if (f32::from(current.y) - f32::from(target_y)).abs() < 0.5 {
            return false;
        }

        base_handle.set_offset(point(current.x, target_y));
        true
    }

    pub(super) fn render_browse_scrollbar(
        &self,
        target: BrowseScrollbarTarget,
        item_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item_count == 0 {
            return div().into_any_element();
        }

        let metrics = self.browse_scrollbar_metrics(target);
        let markers = self.browse_scrollbar_markers(target);
        let current_label = metrics
            .and_then(|metrics| self.current_browse_scrollbar_label(target, metrics, &markers));
        let is_dragging = self
            .browse_scrollbar_drag
            .is_some_and(|drag| drag.target == target);

        div()
            .id(SharedString::from(format!(
                "browse-scrollbar-{}",
                Self::browse_scrollbar_target_id(target)
            )))
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(TABLE_SCROLLBAR_W))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if this.begin_browse_scrollbar_drag(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.drag_browse_scrollbar(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_marker_scrollbar_inner(
                metrics,
                &markers,
                current_label,
                is_dragging,
            ))
            .into_any_element()
    }

    pub(super) fn browse_scrollbar_target_id(target: BrowseScrollbarTarget) -> &'static str {
        match target {
            BrowseScrollbarTarget::ArtistsGrid => "artists-grid",
            BrowseScrollbarTarget::ArtistsTable => "artists-table",
            BrowseScrollbarTarget::AlbumsGrid => "albums-grid",
            BrowseScrollbarTarget::AlbumsTable => "albums-table",
            BrowseScrollbarTarget::GenresGrid => "genres-grid",
            BrowseScrollbarTarget::PlaybackHistory => "playback-history",
            BrowseScrollbarTarget::Liked => "liked",
        }
    }

    fn render_browse_header(
        &self,
        window: &Window,
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
            .when_some(self.render_metadata_status(cx), |this, status| {
                this.child(status)
            })
            .child(div().flex_1())
            .child(self.render_search_box(window, "Search", cx))
            .child(self.with_tooltip(
                self.render_eq_header_button("browse-open-eq", cx),
                "browse-open-eq-tooltip",
                "Equalizer (right-click to toggle)",
                cx,
            ))
            .child(
                self.with_tooltip(
                    self.sidebar_button("⚙", "open-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_page(Page::Settings);
                            cx.notify();
                        })),
                    "browse-open-settings-tooltip",
                    "Settings",
                    cx,
                ),
            )
    }

    pub(super) fn render_view_mode_button(
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
        let border = if active {
            colors.border_strong
        } else {
            colors.border
        };

        div()
            .id(SharedString::from(format!(
                "{}-{}-view",
                page.to_ascii_lowercase(),
                label.to_ascii_lowercase()
            )))
            .w(px(30.0))
            .h_full()
            .border_r_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .border_color(rgb(colors.border_strong))
            })
            .active(|this| this.opacity(0.82))
            .child(Self::view_mode_icon(label, active, colors))
    }

    fn view_mode_icon(label: &'static str, active: bool, colors: ThemeColors) -> AnyElement {
        let color = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let accent = if active { colors.accent } else { color };
        let color = format!("#{:06x}", color);
        let accent = format!("#{:06x}", accent);
        let paths = match label {
            "Grid" => format!(
                r#"<rect x="5" y="5" width="5.4" height="5.4" rx="1" fill="none" stroke="{color}" stroke-width="1.6"/>
<rect x="13.6" y="5" width="5.4" height="5.4" rx="1" fill="none" stroke="{accent}" stroke-width="1.6"/>
<rect x="5" y="13.6" width="5.4" height="5.4" rx="1" fill="none" stroke="{accent}" stroke-width="1.6"/>
<rect x="13.6" y="13.6" width="5.4" height="5.4" rx="1" fill="none" stroke="{color}" stroke-width="1.6"/>"#
            ),
            _ => format!(
                r#"<path d="M6 7H18" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
<path d="M6 12H18" fill="none" stroke="{accent}" stroke-width="1.7" stroke-linecap="round"/>
<path d="M6 17H18" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
<path d="M4 7H4.1M4 12H4.1M4 17H4.1" fill="none" stroke="{accent}" stroke-width="2.2" stroke-linecap="round"/>"#
            ),
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">{paths}</svg>"#
        );

        img(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.into_bytes(),
        )))
        .w(px(15.0))
        .h(px(15.0))
        .flex_none()
        .into_any_element()
    }

    pub(super) fn set_browse_view_mode(&mut self, page: &'static str, mode: BrowseViewMode) {
        let _ = page;
        self.artist_view_mode = mode;
        self.album_view_mode = mode;
        self.genre_view_mode = mode;
    }

    pub(super) fn browse_view_mode(&self) -> BrowseViewMode {
        self.artist_view_mode
    }

    fn render_artist_row(
        &self,
        row_ix: usize,
        artist_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let artist = self.artists.get(artist_ix)?;
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
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .children(
                self.visible_artist_columns
                    .iter()
                    .copied()
                    .map(|column| self.artist_table_cell(column, artist)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_artist_tab(artist_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_album_row(
        &self,
        row_ix: usize,
        album_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let album = self.albums.get(album_ix)?;
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
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .children(
                self.visible_album_columns
                    .iter()
                    .copied()
                    .map(|column| self.album_table_cell(column, album)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_album_tab(album_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_genre_row(
        &self,
        row_ix: usize,
        genre_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let genre = self.genres.get(genre_ix)?;
        let colors = *self.colors();
        let bg = if row_ix.is_multiple_of(2) {
            colors.surface
        } else {
            colors.panel_alt
        };
        let genre_key = genre.key.clone();
        let artists = genre
            .artists
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");

        div()
            .id(SharedString::from(format!("genre-row-{}", genre.key)))
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .children(
                self.visible_genre_columns
                    .iter()
                    .copied()
                    .map(|column| self.genre_table_cell(column, genre, &artists)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_genre_tab(genre_key.clone());
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn artist_table_cell(&self, column: ArtistTableColumn, artist: &Artist) -> AnyElement {
        let colors = *self.colors();
        match column {
            ArtistTableColumn::Artwork => div()
                .w(px(self.artist_table_column_width(column)))
                .child(self.row_image(
                    SharedString::from(format!("artist-row-image-{}", artist.artist_id)),
                    artist.photo_path.as_ref(),
                    artist.initials.clone(),
                    artist.color,
                ))
                .into_any_element(),
            ArtistTableColumn::Artist => div()
                .w(px(self.artist_table_column_width(column)))
                .min_w_0()
                .text_color(rgb(colors.text_strong))
                .overflow_hidden()
                .text_ellipsis()
                .child(artist.name.clone())
                .into_any_element(),
            ArtistTableColumn::Albums => self
                .cell(
                    artist.album_count.to_string(),
                    self.artist_table_column_width(column),
                )
                .into_any_element(),
            ArtistTableColumn::Tracks => self
                .cell(
                    artist.track_count.to_string(),
                    self.artist_table_column_width(column),
                )
                .into_any_element(),
            ArtistTableColumn::Duration => self
                .cell(
                    format_duration_compact(self.artist_total_duration(artist.artist_id)),
                    self.artist_table_column_width(column),
                )
                .into_any_element(),
        }
    }

    fn album_table_cell(&self, column: AlbumTableColumn, album: &Album) -> AnyElement {
        let colors = *self.colors();
        match column {
            AlbumTableColumn::Artwork => div()
                .w(px(self.album_table_column_width(column)))
                .child(self.row_image(
                    SharedString::from(format!(
                        "album-row-image-{}-{}",
                        album.artist_id, album.album_id
                    )),
                    album.artwork_path.as_ref(),
                    album.initials.clone(),
                    album.color,
                ))
                .into_any_element(),
            AlbumTableColumn::Album => div()
                .w(px(self.album_table_column_width(column)))
                .min_w_0()
                .text_color(rgb(colors.text_strong))
                .overflow_hidden()
                .text_ellipsis()
                .child(album.title.clone())
                .into_any_element(),
            AlbumTableColumn::Artist => div()
                .w(px(self.album_table_column_width(column)))
                .min_w_0()
                .text_color(rgb(colors.text_muted))
                .overflow_hidden()
                .text_ellipsis()
                .child(album.artist.clone())
                .into_any_element(),
            AlbumTableColumn::Year => self
                .cell(
                    album.year.clone().unwrap_or_else(|| "Unknown".to_string()),
                    self.album_table_column_width(column),
                )
                .into_any_element(),
            AlbumTableColumn::Tracks => self
                .cell(
                    album.track_count.to_string(),
                    self.album_table_column_width(column),
                )
                .into_any_element(),
            AlbumTableColumn::Duration => self
                .cell(
                    format_duration_compact(self.album_total_duration(album.album_id)),
                    self.album_table_column_width(column),
                )
                .into_any_element(),
        }
    }

    fn genre_table_cell(
        &self,
        column: GenreTableColumn,
        genre: &Genre,
        artists: &str,
    ) -> AnyElement {
        let colors = *self.colors();
        match column {
            GenreTableColumn::Genre => div()
                .w(px(self.genre_table_column_width(column)))
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded_sm()
                        .flex_none()
                        .bg(rgb(genre.color)),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(rgb(colors.text_strong))
                        .child(genre.name.clone()),
                )
                .into_any_element(),
            GenreTableColumn::Artists => self
                .cell(artists.to_string(), self.genre_table_column_width(column))
                .into_any_element(),
            GenreTableColumn::Albums => self
                .cell(
                    genre.album_count.to_string(),
                    self.genre_table_column_width(column),
                )
                .into_any_element(),
            GenreTableColumn::Tracks => self
                .cell(
                    genre.track_count.to_string(),
                    self.genre_table_column_width(column),
                )
                .into_any_element(),
            GenreTableColumn::Duration => self
                .cell(
                    format_duration_compact(genre.duration_value),
                    self.genre_table_column_width(column),
                )
                .into_any_element(),
        }
    }

    fn render_artist_card(
        &self,
        artist: &Artist,
        card_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let artist_id = artist.artist_id;
        if artist.photo_path.is_none() {
            self.queue_artist_metadata_demand(artist_id);
        }

        div()
            .id(SharedString::from(format!(
                "artist-card-{}",
                artist.artist_id
            )))
            .w(px(card_width))
            .flex_none()
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
                card_width,
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

    fn render_genre_card(
        &self,
        genre: &Genre,
        card_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let genre_key = genre.key.clone();
        let play_genre_key = genre.key.clone();
        let bg = blend_rgb(genre.color, colors.surface, 0.66);
        for album in &genre.top_albums {
            if let Some(album_id) = album.album_id
                && album.artwork_path.is_none()
            {
                self.queue_album_cover_demand(album_id);
            }
        }

        div()
            .id(SharedString::from(format!("genre-card-{}", genre.key)))
            .w(px(card_width))
            .h(px(220.0))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(blend_rgb(genre.color, colors.border, 0.45)))
            .bg(rgb(bg))
            .overflow_hidden()
            .cursor_pointer()
            .relative()
            .hover(move |this| {
                this.bg(rgb(blend_rgb(genre.color, colors.hover, 0.55)))
                    .border_color(rgb(blend_rgb(genre.color, colors.border_strong, 0.35)))
            })
            .active(|this| this.opacity(0.9))
            .child(
                div()
                    .h(px(142.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.render_genre_album_fan(
                        SharedString::from(format!("genre-card-fan-{}", genre.key)),
                        &genre.top_albums,
                        genre.color,
                        86.0,
                    )),
            )
            .child(
                div()
                    .px_4()
                    .pb_4()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_strong))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(genre.name.clone()),
                            )
                            .child(div().text_xs().text_color(rgb(colors.text_muted)).child(
                                format!(
                                    "{} albums  ·  {} tracks",
                                    genre.album_count, genre.track_count
                                ),
                            )),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("genre-play-{}", genre.key)))
                            .w(px(30.0))
                            .h(px(30.0))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(blend_rgb(genre.color, colors.text_strong, 0.18)))
                            .text_color(rgb(colors.surface))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child("▶")
                            .on_click(cx.listener(
                                move |this, _event: &ClickEvent, _window, cx| {
                                    this.play_genre(&play_genre_key, false, cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                },
                            )),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_genre_tab(genre_key.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_album_card(
        &self,
        album: &Album,
        card_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let album_id = album.album_id;
        if album.artwork_path.is_none() {
            self.queue_album_cover_demand(album_id);
        }

        div()
            .id(SharedString::from(format!(
                "album-card-{}-{}",
                album.artist_id, album.album_id
            )))
            .w(px(card_width))
            .flex_none()
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
                card_width,
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

    fn render_artist_album_strip(&self, albums: &[&Album], cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("artist-album-hero-strip")
            .w_full()
            .overflow_x_scroll()
            .child(
                div().flex().gap_3().pb_2().children(
                    albums
                        .iter()
                        .map(|album| self.render_album_card(album, BROWSE_GRID_CARD_W, cx)),
                ),
            )
            .into_any_element()
    }

    fn render_genre_artist_pills(&self, genre: &Genre) -> AnyElement {
        let colors = *self.colors();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_faint))
                    .child("ARTISTS"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(genre.artists.iter().take(16).map(|artist| {
                        div()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.button))
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(colors.text))
                            .child(artist.clone())
                    })),
            )
            .into_any_element()
    }

    fn render_genre_album_strip(&self, genre: &Genre, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();

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
            .child(
                div()
                    .id(SharedString::from(format!(
                        "genre-album-strip-scroll-{}",
                        genre.key
                    )))
                    .w_full()
                    .overflow_x_scroll()
                    .child(
                        div().flex().gap_3().pb_2().children(
                            genre
                                .albums
                                .iter()
                                .take(12)
                                .map(|album| self.render_genre_album_card(album, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_genre_album_card(
        &self,
        album: &GenreAlbumSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let album_id = album.album_id;
        if let Some(album_id) = album_id
            && album.artwork_path.is_none()
        {
            self.queue_album_cover_demand(album_id);
        }

        div()
            .id(SharedString::from(format!(
                "genre-album-card-{}-{}",
                album.artist, album.title
            )))
            .w(px(BROWSE_GRID_CARD_W))
            .flex_none()
            .cursor_pointer()
            .child(
                div()
                    .w(px(BROWSE_GRID_CARD_W))
                    .h(px(BROWSE_GRID_CARD_W))
                    .relative()
                    .child(self.render_genre_album_fan_card(
                        SharedString::from(format!("genre-album-card-image-{}", album.title)),
                        album,
                        BROWSE_GRID_CARD_W,
                        0.0,
                        0.0,
                    )),
            )
            .child(
                div()
                    .pt_2()
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
                            .child(format!("{}  ·  {} tracks", album.artist, album.track_count)),
                    ),
            )
            .when_some(album_id, |this, album_id| {
                this.on_click(cx.listener(move |this, _, _, cx| {
                    this.open_album_tab(album_id);
                    cx.notify();
                }))
            })
            .into_any_element()
    }

    fn genre_action_button(&self, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
        let bg = if primary {
            colors.accent
        } else {
            colors.button
        };
        let fg = if primary { colors.surface } else { colors.text };

        div()
            .id(SharedString::from(format!("genre-action-{}", label)))
            .h(px(30.0))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |this| {
                this.bg(rgb(if primary {
                    colors.accent_soft
                } else {
                    colors.button_hover
                }))
            })
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    fn render_genre_album_fan(
        &self,
        id: SharedString,
        albums: &[GenreAlbumSummary],
        fallback_color: u32,
        cover_size: f32,
    ) -> AnyElement {
        let fan_width = cover_size * 2.35;
        let fan_height = cover_size * 1.35;
        let cards = if albums.is_empty() {
            vec![GenreAlbumSummary {
                album_id: None,
                title: "Genre".to_string(),
                artist: String::new(),
                artwork_path: None,
                track_count: 0,
                play_count: 0,
                initials: "#".to_string(),
                color: fallback_color,
            }]
        } else {
            albums.iter().take(3).cloned().collect::<Vec<_>>()
        };
        let count = cards.len();

        div()
            .id(id)
            .w(px(fan_width))
            .h(px(fan_height))
            .relative()
            .children(cards.into_iter().enumerate().map(|(ix, album)| {
                let (left, top) = match (count, ix) {
                    (1, _) => (fan_width / 2.0 - cover_size / 2.0, fan_height * 0.1),
                    (2, 0) => (fan_width * 0.22 - cover_size / 2.0, fan_height * 0.16),
                    (2, _) => (fan_width * 0.62 - cover_size / 2.0, fan_height * 0.04),
                    (_, 0) => (fan_width * 0.18 - cover_size / 2.0, fan_height * 0.2),
                    (_, 1) => (fan_width * 0.42 - cover_size / 2.0, fan_height * 0.08),
                    (_, _) => (fan_width * 0.68 - cover_size / 2.0, fan_height * 0.16),
                };
                self.render_genre_album_fan_card(
                    SharedString::from(format!("genre-fan-card-{}-{}", album.title, ix)),
                    &album,
                    cover_size,
                    left,
                    top,
                )
            }))
            .into_any_element()
    }

    fn render_genre_album_fan_card(
        &self,
        id: SharedString,
        album: &GenreAlbumSummary,
        size: f32,
        left: f32,
        top: f32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = album.initials.clone();

        div()
            .id(id)
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(size))
            .h(px(size))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .overflow_hidden()
            .shadow_lg()
            .child(match &album.artwork_path {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback({
                        let initials = fallback_initials.clone();
                        let color = album.color;
                        move || artwork::album_tile_fallback(initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => artwork::album_tile_fallback(album.initials.clone(), album.color, colors),
            })
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
                        artwork::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => artwork::album_tile_fallback(initials, color, colors),
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
                        artwork::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => artwork::album_tile_fallback(initials, color, colors),
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
            .w(px(22.0))
            .h(px(22.0))
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
                        artwork::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => artwork::album_tile_fallback(initials, color, colors),
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
