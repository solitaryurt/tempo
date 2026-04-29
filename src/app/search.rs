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
                SortColumn::Album => left
                    .album
                    .cmp(&right.album)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Format => left
                    .codec
                    .cmp(&right.codec)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Plays => left
                    .plays
                    .parse::<u32>()
                    .unwrap_or_default()
                    .cmp(&right.plays.parse::<u32>().unwrap_or_default()),
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
            SortColumn::Title | SortColumn::Album | SortColumn::Format => {
                self.compute_grouped_scrollbar_markers(indices, sort_column)
            }
            SortColumn::Index | SortColumn::Plays | SortColumn::Duration => {
                self.compute_sampled_scrollbar_markers(indices, sort_column)
            }
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
                SortColumn::Album => Self::marker_initial(&track.album),
                SortColumn::Format => track.codec.to_ascii_uppercase(),
                SortColumn::Index | SortColumn::Plays | SortColumn::Duration => unreachable!(),
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
            SortColumn::Album => Self::marker_initial(&track.album),
            SortColumn::Format => track.codec.to_ascii_uppercase(),
            SortColumn::Plays => track.plays.clone(),
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
        }
    }

    pub(super) fn source_track_count(&self, source: TabSource) -> usize {
        match source {
            TabSource::Library => self.tracks.len(),
            TabSource::Playlist(_) => self.source_track_indices(source).len(),
        }
    }

    pub(super) fn set_search_query(&mut self, query: String) {
        self.active_tab_mut().search_query = query;
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
        if !self.active_search_query().is_empty() {
            self.set_search_query(String::new());
        }
    }

    pub(super) fn handle_search_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        match event.keystroke.key.as_str() {
            "backspace" => {
                let mut query = self.active_search_query().to_string();
                query.pop();
                self.set_search_query(query);
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.clear_search_query();
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                let Some(key_char) = event.keystroke.key_char.as_ref() else {
                    return;
                };
                if key_char.chars().all(|ch| !ch.is_control()) {
                    let mut query = self.active_search_query().to_string();
                    query.push_str(key_char);
                    self.set_search_query(query);
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
}
