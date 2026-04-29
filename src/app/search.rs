use super::*;

impl TempoApp {
    pub(super) fn filtered_track_count(&self) -> usize {
        self.current_track_indices().len()
    }

    pub(super) fn active_source_track_count(&self) -> usize {
        self.source_track_count(self.active_tab().source)
    }

    pub(super) fn invalidate_track_indices(&mut self) {
        for tab_ix in 0..self.tabs.len() {
            self.rebuild_track_indices_for_tab(tab_ix);
        }
    }

    pub(super) fn current_track_indices(&self) -> &[usize] {
        self.track_indices_for_tab(self.active_tab)
    }

    pub(super) fn track_indices_for_tab(&self, tab_ix: usize) -> &[usize] {
        self.tabs
            .get(tab_ix)
            .map(|tab| tab.track_indices.as_slice())
            .unwrap_or_default()
    }

    pub(super) fn rebuild_track_indices_for_tab(&mut self, tab_ix: usize) {
        let Some(tab) = self.tabs.get(tab_ix) else {
            return;
        };

        let source = tab.source;
        let search_query = tab.search_query.clone();
        let sort_column = tab.sort_column;
        let sort_direction = tab.sort_direction;

        let indices =
            self.compute_track_indices(source, &search_query, sort_column, sort_direction);
        let scrollbar_markers = self.compute_scrollbar_markers(&indices, sort_column);
        if let Some(tab) = self.tabs.get_mut(tab_ix) {
            tab.track_indices = indices;
            tab.scrollbar_markers = scrollbar_markers;
        }
    }

    pub(super) fn compute_track_indices(
        &self,
        source: TabSource,
        search_query: &str,
        sort_column: SortColumn,
        sort_direction: SortDirection,
    ) -> Vec<usize> {
        let terms = Self::search_terms(search_query);
        let mut indices = self
            .source_track_indices(source)
            .into_iter()
            .filter(|track_ix| {
                self.tracks
                    .get(*track_ix)
                    .is_some_and(|track| Self::track_matches_search_terms(track, &terms))
            })
            .collect::<Vec<_>>();

        if sort_column == SortColumn::Index {
            if sort_direction == SortDirection::Descending {
                indices.reverse();
            }
            return indices;
        }

        indices.sort_by(|a, b| {
            let left = &self.tracks[*a];
            let right = &self.tracks[*b];
            let ordering = match sort_column {
                SortColumn::Index => a.cmp(b),
                SortColumn::Title => left.title.cmp(&right.title),
                SortColumn::Artist => left
                    .artist
                    .cmp(&right.artist)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Album => left
                    .album
                    .cmp(&right.album)
                    .then(left.title.cmp(&right.title)),
                SortColumn::TrackNumber => left
                    .track_number
                    .cmp(&right.track_number)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Format => left
                    .codec
                    .cmp(&right.codec)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Bitrate => left.bitrate.cmp(&right.bitrate),
                SortColumn::FileSize => left.file_size.cmp(&right.file_size),
                SortColumn::Year => left
                    .year
                    .cmp(&right.year)
                    .then(left.title.cmp(&right.title)),
                SortColumn::DateAdded => left.date_added.cmp(&right.date_added),
                SortColumn::Plays => left.plays.cmp(&right.plays),
                SortColumn::Duration => left.duration_value.cmp(&right.duration_value),
            };

            match sort_direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });

        indices
    }

    pub(super) fn compute_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        if indices.len() <= 1 {
            return Vec::new();
        }

        match sort_column {
            SortColumn::Title
            | SortColumn::Artist
            | SortColumn::Album
            | SortColumn::Format
            | SortColumn::Year => self.compute_grouped_scrollbar_markers(indices, sort_column),
            SortColumn::Index
            | SortColumn::TrackNumber
            | SortColumn::Bitrate
            | SortColumn::FileSize
            | SortColumn::DateAdded
            | SortColumn::Plays
            | SortColumn::Duration => self.compute_sampled_scrollbar_markers(indices, sort_column),
        }
    }

    pub(super) fn compute_grouped_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        let denominator = indices.len().saturating_sub(1) as f32;
        let mut markers = Vec::new();
        let mut previous_label = String::new();

        for (row_ix, track_ix) in indices.iter().copied().enumerate() {
            let Some(track) = self.tracks.get(track_ix) else {
                continue;
            };
            let label = match sort_column {
                SortColumn::Title => Self::marker_initial(&track.title),
                SortColumn::Artist => Self::marker_initial(&track.artist),
                SortColumn::Album => Self::marker_initial(&track.album),
                SortColumn::Format => track.codec.to_ascii_uppercase(),
                SortColumn::Year => Self::marker_initial(&track.year),
                SortColumn::Index
                | SortColumn::TrackNumber
                | SortColumn::Bitrate
                | SortColumn::FileSize
                | SortColumn::DateAdded
                | SortColumn::Plays
                | SortColumn::Duration => unreachable!(),
            };

            if label == previous_label {
                continue;
            }

            previous_label = label.clone();
            markers.push(ScrollbarMarker {
                ratio: row_ix as f32 / denominator,
                label,
            });

            if markers.len() >= TABLE_SCROLLBAR_MAX_MARKERS {
                break;
            }
        }

        markers
    }

    pub(super) fn compute_sampled_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        let samples = [0.0_f32, 0.25, 0.5, 0.75, 1.0];
        let last_row = indices.len().saturating_sub(1);
        let mut markers = Vec::new();

        for ratio in samples {
            let row_ix = (ratio * last_row as f32).round() as usize;
            let Some(track_ix) = indices.get(row_ix).copied() else {
                continue;
            };
            let label = self.scrollbar_marker_label(track_ix, sort_column);
            if markers
                .iter()
                .any(|marker: &ScrollbarMarker| marker.label == label)
            {
                continue;
            }

            markers.push(ScrollbarMarker { ratio, label });
        }

        markers
    }

    pub(super) fn marker_initial(value: &str) -> String {
        value
            .trim_start()
            .chars()
            .find(|ch| ch.is_alphanumeric())
            .map(|ch| ch.to_uppercase().collect::<String>())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| "#".to_string())
    }

    pub(super) fn scrollbar_marker_label(
        &self,
        track_ix: usize,
        sort_column: SortColumn,
    ) -> String {
        let Some(track) = self.tracks.get(track_ix) else {
            return String::new();
        };

        match sort_column {
            SortColumn::Index => format!("{}", track_ix + 1),
            SortColumn::Title => Self::marker_initial(&track.title),
            SortColumn::Artist => Self::marker_initial(&track.artist),
            SortColumn::Album => Self::marker_initial(&track.album),
            SortColumn::TrackNumber => track
                .track_number
                .map(|track_number| track_number.to_string())
                .unwrap_or_else(|| "-".to_string()),
            SortColumn::Format => track.codec.to_ascii_uppercase(),
            SortColumn::Bitrate => TempoApp::bitrate_cell_label(track),
            SortColumn::FileSize => TempoApp::file_size_label(track.file_size),
            SortColumn::Year => Self::marker_initial(&track.year),
            SortColumn::DateAdded => TempoApp::date_label(track.date_added),
            SortColumn::Plays => track.plays.to_string(),
            SortColumn::Duration => track.duration.clone(),
        }
    }

    pub(super) fn source_track_indices(&self, source: TabSource) -> Vec<usize> {
        match source {
            TabSource::Library => (0..self.tracks.len()).collect(),
            TabSource::Playlist(playlist_ix) => self
                .playlists
                .get(playlist_ix)
                .map(|playlist| {
                    playlist
                        .track_paths
                        .iter()
                        .filter_map(|path| self.tracks.iter().position(|track| track.path == *path))
                        .collect()
                })
                .unwrap_or_default(),
            TabSource::Artist(artist_id) => {
                let artist_name = self
                    .artist_by_id(artist_id)
                    .map(|artist| artist.name.as_str());
                self.tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, track)| {
                        (track.artist_id == Some(artist_id)
                            || artist_name.is_some_and(|name| {
                                individual_artist_names(&track.artist)
                                    .iter()
                                    .any(|artist| artist == name)
                            }))
                        .then_some(ix)
                    })
                    .collect()
            }
            TabSource::Album(album_id) => {
                let album = self.album_by_id(album_id);
                self.tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, track)| {
                        (track.album_id == Some(album_id)
                            || album.is_some_and(|album| {
                                track.album == album.title
                                    && primary_artist_name(&track.artist) == album.artist
                            }))
                        .then_some(ix)
                    })
                    .collect()
            }
        }
    }

    pub(super) fn source_track_count(&self, source: TabSource) -> usize {
        match source {
            TabSource::Library => self.tracks.len(),
            TabSource::Playlist(_) | TabSource::Artist(_) | TabSource::Album(_) => {
                self.source_track_indices(source).len()
            }
        }
    }

    pub(super) fn set_search_query(&mut self, query: String) {
        self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);
        self.active_tab_mut().search_query = query.clone();
        self.search_input.set_text(query);
        self.context_menu_track = None;
        self.invalidate_track_indices();
        let selected_track = self.active_selected_track();
        let replacement_track = {
            let indices = self.current_track_indices();
            if let Some(first_track_ix) = indices.first() {
                (!indices.contains(&selected_track)).then_some(*first_track_ix)
            } else {
                Some(0)
            }
        };
        if let Some(track_ix) = replacement_track {
            self.set_active_selected_track(track_ix);
        }
        self.active_tab()
            .table_scroll_handle
            .scroll_to_item(0, ScrollStrategy::Top);
    }

    pub(super) fn clear_search_query(&mut self) {
        self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);
        if self.page == Page::Library {
            if !self.active_search_query().is_empty() {
                self.set_search_query(String::new());
            } else if !self.search_input.text().is_empty() {
                self.search_input.clear();
            }
        } else {
            self.search_input.clear();
        }
        self.browse_search_query.clear();
    }

    fn should_live_filter_active_tab(&self) -> bool {
        self.page == Page::Library
            && (self.active_tab().source == TabSource::Library
                || !self.active_search_query().trim().is_empty())
    }

    fn schedule_current_search_input(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.text().to_string();
        if self.should_live_filter_active_tab() || matches!(self.page, Page::Artists | Page::Albums)
        {
            self.schedule_search_apply(query, cx);
        }
    }

    fn schedule_search_apply(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);
        let generation = self.search_debounce_generation;
        let target_page = self.page;
        let target_tab = self.active_tab;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE_DELAY).await;

            let Ok(()) = this.update(cx, |app, cx| {
                if app.search_debounce_generation != generation || app.page != target_page {
                    return;
                }

                match target_page {
                    Page::Library => {
                        if app.active_tab == target_tab && app.should_live_filter_active_tab() {
                            app.set_search_query(query);
                            cx.notify();
                        }
                    }
                    Page::Artists | Page::Albums => {
                        if app.browse_search_query != query {
                            app.browse_search_query = query;
                            cx.notify();
                        }
                    }
                    Page::ScanErrors | Page::Settings => {}
                }
            }) else {
                return;
            };
        })
        .detach();
    }

    fn submit_search(&mut self, new_tab: bool, force_current_tab: bool) {
        let query = self.search_input.text().trim().to_string();
        if query.is_empty() {
            return;
        }

        self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);

        if new_tab {
            let previous_tab = self.active_tab;
            self.new_search_tab(query);
            if self
                .tabs
                .get(previous_tab)
                .is_some_and(|tab| tab.source == TabSource::Library)
            {
                if let Some(tab) = self.tabs.get_mut(previous_tab) {
                    tab.search_query.clear();
                }
                self.rebuild_track_indices_for_tab(previous_tab);
            }
        } else if force_current_tab || self.active_tab().source == TabSource::Library {
            self.open_page(Page::Library);
            self.set_search_query(query);
        } else {
            self.open_all_music_tab();
            self.set_search_query(query);
        }
    }

    pub(super) fn handle_search_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;

        match event.keystroke.key.as_str() {
            "enter" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.submit_search(modifiers.control || modifiers.platform, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "backspace" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.backspace(command);
                self.schedule_current_search_input(cx);
                cx.stop_propagation();
                cx.notify();
            }
            "delete" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.delete(command);
                self.schedule_current_search_input(cx);
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.clear_search_query();
                cx.stop_propagation();
                cx.notify();
            }
            "left" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.move_left(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "right" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.move_right(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "home" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.move_home(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "end" => {
                if modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.move_end(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "space" => {
                if command || modifiers.alt || modifiers.function {
                    return;
                }
                self.search_input.insert(" ");
                self.schedule_current_search_input(cx);
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                if command && !modifiers.alt && !modifiers.function {
                    match event.keystroke.key.as_str() {
                        "a" => {
                            self.search_input.select_all();
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        "c" => {
                            if let Some(text) = self.search_input.selected_text() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                            cx.stop_propagation();
                            return;
                        }
                        "x" => {
                            if let Some(text) = self.search_input.selected_text() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                self.search_input.insert("");
                                self.schedule_current_search_input(cx);
                                cx.notify();
                            }
                            cx.stop_propagation();
                            return;
                        }
                        "v" => {
                            if let Some(text) =
                                cx.read_from_clipboard().and_then(|item| item.text())
                            {
                                self.search_input.insert(&text.replace('\n', " "));
                                self.schedule_current_search_input(cx);
                                cx.notify();
                            }
                            cx.stop_propagation();
                            return;
                        }
                        _ => {}
                    }
                }

                let Some(key_char) = event.keystroke.key_char.as_ref() else {
                    return;
                };
                if command || modifiers.alt || modifiers.function {
                    return;
                }
                if key_char.chars().all(|ch| !ch.is_control()) {
                    self.search_input.insert(key_char);
                    self.schedule_current_search_input(cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn search_terms(query: &str) -> Vec<String> {
        query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect()
    }

    pub(super) fn track_matches_search_terms(track: &Track, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }

        let searchable = format!(
            "{} {} {} {} {} {}",
            track.title,
            track.artist,
            track.album,
            track.year,
            track.codec,
            track.path.display()
        )
        .to_lowercase();

        terms.iter().all(|term| searchable.contains(term))
    }

    pub(super) fn artist_indices_for_search_query(&self, query: &str) -> Vec<usize> {
        let terms = Self::search_terms(query);
        self.artists
            .iter()
            .enumerate()
            .filter_map(|(ix, artist)| {
                Self::artist_matches_search_terms(artist, &terms).then_some(ix)
            })
            .collect()
    }

    pub(super) fn album_indices_for_search_query(&self, query: &str) -> Vec<usize> {
        let terms = Self::search_terms(query);
        self.albums
            .iter()
            .enumerate()
            .filter_map(|(ix, album)| Self::album_matches_search_terms(album, &terms).then_some(ix))
            .collect()
    }

    pub(super) fn artist_matches_search_terms(artist: &Artist, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }

        let searchable = format!(
            "{} {} {} {}",
            artist.name,
            artist.bio.as_deref().unwrap_or_default(),
            artist.album_count,
            artist.track_count
        )
        .to_lowercase();

        terms.iter().all(|term| searchable.contains(term))
    }

    pub(super) fn album_matches_search_terms(album: &Album, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }

        let searchable = format!(
            "{} {} {} {}",
            album.title,
            album.artist,
            album.year.as_deref().unwrap_or_default(),
            album.track_count
        )
        .to_lowercase();

        terms.iter().all(|term| searchable.contains(term))
    }
}
