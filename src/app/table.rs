use super::*;
use std::time::Instant;

const AUTOSIZE_TEXT_PADDING: f32 = 20.0;
const AUTOSIZE_TEXT_CHAR_W: f32 = 7.2;
const AUTOSIZE_ICON_COL_W: f32 = 34.0;
const TABLE_HORIZONTAL_SCROLLBAR_H: f32 = 14.0;
const TABLE_HORIZONTAL_SCROLLBAR_TRACK_H: f32 = 6.0;
const TABLE_HORIZONTAL_SCROLLBAR_MARGIN: f32 = 4.0;
const TABLE_HORIZONTAL_SCROLLBAR_MIN_THUMB_W: f32 = 32.0;

impl TempoApp {
    pub(super) fn column_width(&self, column: TableColumn) -> f32 {
        self.resize_target_width(ColumnResizeTarget::Track(column))
    }

    pub(super) fn resize_target_width(&self, target: ColumnResizeTarget) -> f32 {
        match target {
            ColumnResizeTarget::Track(column) => self.track_column_width(column),
            ColumnResizeTarget::Artist(column) => self.artist_table_column_width(column),
            ColumnResizeTarget::Album(column) => self.album_table_column_width(column),
            ColumnResizeTarget::ScanError(column) => self.scan_error_column_width(column),
            ColumnResizeTarget::PlaybackHistoryPlayedAt => self.playback_history_played_at_width,
        }
    }

    fn track_column_width(&self, column: TableColumn) -> f32 {
        match column {
            TableColumn::Index => self.column_widths.index,
            TableColumn::Artwork => self.column_widths.artwork,
            TableColumn::Title => self.column_widths.title,
            TableColumn::Artist => self.column_widths.artist,
            TableColumn::Album => self.column_widths.album,
            TableColumn::TrackNumber => self.column_widths.track_number,
            TableColumn::Format => self.column_widths.format,
            TableColumn::Bitrate => self.column_widths.bitrate,
            TableColumn::FileSize => self.column_widths.file_size,
            TableColumn::Year => self.column_widths.year,
            TableColumn::DateAdded => self.column_widths.date_added,
            TableColumn::Plays => self.column_widths.plays,
            TableColumn::Duration => self.column_widths.duration,
            TableColumn::Loved => self.column_widths.loved,
        }
    }

    pub(super) fn artist_table_column_width(&self, column: ArtistTableColumn) -> f32 {
        match column {
            ArtistTableColumn::Artwork => self.artist_table_column_widths.artwork,
            ArtistTableColumn::Artist => self.artist_table_column_widths.artist,
            ArtistTableColumn::Albums => self.artist_table_column_widths.albums,
            ArtistTableColumn::Tracks => self.artist_table_column_widths.tracks,
        }
    }

    pub(super) fn album_table_column_width(&self, column: AlbumTableColumn) -> f32 {
        match column {
            AlbumTableColumn::Artwork => self.album_table_column_widths.artwork,
            AlbumTableColumn::Album => self.album_table_column_widths.album,
            AlbumTableColumn::Artist => self.album_table_column_widths.artist,
            AlbumTableColumn::Year => self.album_table_column_widths.year,
            AlbumTableColumn::Tracks => self.album_table_column_widths.tracks,
        }
    }

    pub(super) fn scan_error_column_width(&self, column: ScanErrorColumn) -> f32 {
        match column {
            ScanErrorColumn::Index => self.scan_error_column_widths.index,
            ScanErrorColumn::Path => self.scan_error_column_widths.path,
            ScanErrorColumn::Error => self.scan_error_column_widths.error,
        }
    }

    pub(super) fn set_resize_target_width(&mut self, target: ColumnResizeTarget, width: f32) {
        let width = width.max(Self::min_resize_target_width(target));
        match target {
            ColumnResizeTarget::Track(column) => self.set_track_column_width(column, width),
            ColumnResizeTarget::Artist(column) => match column {
                ArtistTableColumn::Artwork => self.artist_table_column_widths.artwork = width,
                ArtistTableColumn::Artist => self.artist_table_column_widths.artist = width,
                ArtistTableColumn::Albums => self.artist_table_column_widths.albums = width,
                ArtistTableColumn::Tracks => self.artist_table_column_widths.tracks = width,
            },
            ColumnResizeTarget::Album(column) => match column {
                AlbumTableColumn::Artwork => self.album_table_column_widths.artwork = width,
                AlbumTableColumn::Album => self.album_table_column_widths.album = width,
                AlbumTableColumn::Artist => self.album_table_column_widths.artist = width,
                AlbumTableColumn::Year => self.album_table_column_widths.year = width,
                AlbumTableColumn::Tracks => self.album_table_column_widths.tracks = width,
            },
            ColumnResizeTarget::ScanError(column) => match column {
                ScanErrorColumn::Index => self.scan_error_column_widths.index = width,
                ScanErrorColumn::Path => self.scan_error_column_widths.path = width,
                ScanErrorColumn::Error => self.scan_error_column_widths.error = width,
            },
            ColumnResizeTarget::PlaybackHistoryPlayedAt => {
                self.playback_history_played_at_width = width
            }
        }
    }

    fn set_track_column_width(&mut self, column: TableColumn, width: f32) {
        match column {
            TableColumn::Index => self.column_widths.index = width,
            TableColumn::Artwork => self.column_widths.artwork = width,
            TableColumn::Title => self.column_widths.title = width,
            TableColumn::Artist => self.column_widths.artist = width,
            TableColumn::Album => self.column_widths.album = width,
            TableColumn::TrackNumber => self.column_widths.track_number = width,
            TableColumn::Format => self.column_widths.format = width,
            TableColumn::Bitrate => self.column_widths.bitrate = width,
            TableColumn::FileSize => self.column_widths.file_size = width,
            TableColumn::Year => self.column_widths.year = width,
            TableColumn::DateAdded => self.column_widths.date_added = width,
            TableColumn::Plays => self.column_widths.plays = width,
            TableColumn::Duration => self.column_widths.duration = width,
            TableColumn::Loved => self.column_widths.loved = width,
        }

        self.clamp_table_horizontal_scroll();
    }

    pub(super) fn table_content_width(&self) -> f32 {
        self.visible_columns
            .iter()
            .copied()
            .map(|column| self.column_width(column))
            .sum()
    }

    pub(super) fn table_render_width(&self) -> f32 {
        self.table_horizontal_viewport_width()
            .map(|viewport_width| self.table_content_width().max(viewport_width))
            .unwrap_or_else(|| self.table_content_width())
    }

    pub(super) fn table_column_render_width(&self, column: TableColumn) -> f32 {
        let width = self.column_width(column);
        if self.visible_columns.last().copied() != Some(column) {
            return width;
        }

        let extra_width = self
            .table_horizontal_viewport_width()
            .map(|viewport_width| (viewport_width - self.table_content_width()).max(0.0))
            .unwrap_or(0.0);
        width + extra_width
    }

    pub(super) fn table_horizontal_viewport_width(&self) -> Option<f32> {
        let handle = self.active_tab().table_scroll_handle.clone();
        let base_handle = handle.0.borrow().base_handle.clone();
        let width = f32::from(base_handle.bounds().size.width) - 32.0 - TABLE_SCROLLBAR_W;
        (width > 0.0).then_some(width)
    }

    pub(super) fn max_table_horizontal_scroll(&self) -> f32 {
        let Some(viewport_width) = self.table_horizontal_viewport_width() else {
            return 0.0;
        };

        (self.table_content_width() - viewport_width).max(0.0)
    }

    pub(super) fn current_table_horizontal_scroll(&self) -> f32 {
        self.active_tab()
            .table_horizontal_scroll
            .clamp(0.0, self.max_table_horizontal_scroll())
    }

    pub(super) fn clamp_table_horizontal_scroll(&mut self) {
        let scroll = self.current_table_horizontal_scroll();
        self.active_tab_mut().table_horizontal_scroll = scroll;
    }

    pub(super) fn scroll_table_horizontally(&mut self, delta: f32) -> bool {
        let max_scroll = self.max_table_horizontal_scroll();
        if max_scroll <= 0.0 {
            self.active_tab_mut().table_horizontal_scroll = 0.0;
            return false;
        }

        let current = self.active_tab().table_horizontal_scroll;
        let next = (current + delta).clamp(0.0, max_scroll);
        if (next - current).abs() < 0.5 {
            return false;
        }

        self.active_tab_mut().table_horizontal_scroll = next;
        perf::event(
            "table.scroll.horizontal",
            format!("delta={delta:.1} offset={next:.1} max={max_scroll:.1}"),
        );
        true
    }

    pub(super) fn handle_table_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.shift {
            let delta = event.delta.pixel_delta(px(TABLE_ROW_H));
            let x = f32::from(delta.x);
            let y = f32::from(delta.y);
            let scroll_delta = if x.abs() > y.abs() { x } else { y };

            if self.scroll_table_horizontally(scroll_delta) {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        self.mark_table_scrolling(cx);
    }

    pub(super) fn min_resize_target_width(target: ColumnResizeTarget) -> f32 {
        match target {
            ColumnResizeTarget::Track(column) => Self::min_track_column_width(column),
            ColumnResizeTarget::Artist(ArtistTableColumn::Artwork)
            | ColumnResizeTarget::Album(AlbumTableColumn::Artwork) => 34.0,
            ColumnResizeTarget::Artist(ArtistTableColumn::Artist)
            | ColumnResizeTarget::Album(AlbumTableColumn::Album)
            | ColumnResizeTarget::Album(AlbumTableColumn::Artist)
            | ColumnResizeTarget::ScanError(ScanErrorColumn::Path)
            | ColumnResizeTarget::ScanError(ScanErrorColumn::Error) => 96.0,
            ColumnResizeTarget::Artist(ArtistTableColumn::Albums)
            | ColumnResizeTarget::Artist(ArtistTableColumn::Tracks)
            | ColumnResizeTarget::Album(AlbumTableColumn::Year)
            | ColumnResizeTarget::Album(AlbumTableColumn::Tracks)
            | ColumnResizeTarget::ScanError(ScanErrorColumn::Index) => 52.0,
            ColumnResizeTarget::PlaybackHistoryPlayedAt => 120.0,
        }
    }

    fn min_track_column_width(column: TableColumn) -> f32 {
        match column {
            TableColumn::Index | TableColumn::Artwork | TableColumn::Loved => 24.0,
            TableColumn::Format => 44.0,
            TableColumn::TrackNumber | TableColumn::Plays | TableColumn::Duration => 52.0,
            TableColumn::Bitrate | TableColumn::FileSize | TableColumn::Year => 60.0,
            TableColumn::Title | TableColumn::Artist | TableColumn::Album => 96.0,
            TableColumn::DateAdded => 82.0,
        }
    }

    pub(super) fn begin_column_resize(&mut self, column: TableColumn, event: &MouseDownEvent) {
        self.begin_resize_target(ColumnResizeTarget::Track(column), event);
    }

    pub(super) fn begin_resize_target(
        &mut self,
        target: ColumnResizeTarget,
        event: &MouseDownEvent,
    ) {
        perf::event(
            "table.resize.begin",
            format!(
                "target={} width={:.1}",
                Self::resize_target_label(target),
                self.resize_target_width(target)
            ),
        );
        self.column_resize = Some(ColumnResize {
            target,
            start_x: f32::from(event.position.x),
            start_width: self.resize_target_width(target),
        });
        self.context_menu_track = None;
    }

    pub(super) fn resize_column_from_mouse(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(resize) = self.column_resize else {
            return false;
        };

        if !event.dragging() {
            self.column_resize = None;
            return false;
        }

        let delta = f32::from(event.position.x) - resize.start_x;
        self.set_resize_target_width(resize.target, resize.start_width + delta);
        true
    }

    pub(super) fn finish_column_resize(&mut self) -> bool {
        let resize = self.column_resize.take();
        if let Some(resize) = resize {
            perf::event(
                "table.resize.finish",
                format!(
                    "target={} width={:.1}",
                    Self::resize_target_label(resize.target),
                    self.resize_target_width(resize.target)
                ),
            );
            true
        } else {
            false
        }
    }

    pub(super) fn autosize_table_column(&mut self, column: TableColumn) {
        self.autosize_resize_target(ColumnResizeTarget::Track(column));
    }

    pub(super) fn autosize_resize_target(&mut self, target: ColumnResizeTarget) {
        let start = Instant::now();
        self.column_resize = None;
        self.set_resize_target_width(target, self.autosize_resize_target_width(target));
        perf::log_duration(
            "table.autosize_column",
            start.elapsed(),
            format!("target={}", Self::resize_target_label(target)),
        );
    }

    pub(super) fn autosize_resize_target_width(&self, target: ColumnResizeTarget) -> f32 {
        let start = Instant::now();
        match target {
            ColumnResizeTarget::Track(column) => self.autosize_table_column_width(column),
            ColumnResizeTarget::Artist(ArtistTableColumn::Artwork)
            | ColumnResizeTarget::Album(AlbumTableColumn::Artwork) => AUTOSIZE_ICON_COL_W,
            _ => {
                let header_width = Self::autosize_text_width(Self::resize_target_label(target));
                let content_width = self
                    .resize_target_texts(target)
                    .into_iter()
                    .map(|text| Self::autosize_text_width(&text))
                    .fold(header_width, f32::max);

                let width = content_width + AUTOSIZE_TEXT_PADDING;
                perf::log_duration_if_slow(
                    "table.autosize_column_width",
                    start.elapsed(),
                    Duration::from_millis(4),
                    format!(
                        "target={} width={width:.1}",
                        Self::resize_target_label(target)
                    ),
                );
                width
            }
        }
    }

    pub(super) fn autosize_table_column_width(&self, column: TableColumn) -> f32 {
        let start = Instant::now();
        match column {
            TableColumn::Artwork | TableColumn::Loved => return AUTOSIZE_ICON_COL_W,
            _ => {}
        }

        let header_width = Self::autosize_text_width(Self::column_label(column));
        let content_width = self
            .current_track_indices()
            .iter()
            .filter_map(|track_ix| self.tracks.get(*track_ix).map(|track| (track, *track_ix)))
            .map(|(track, track_ix)| {
                Self::autosize_text_width(&self.table_column_text(column, track_ix, track))
            })
            .fold(header_width, f32::max);

        let width = content_width + AUTOSIZE_TEXT_PADDING;
        perf::log_duration_if_slow(
            "table.autosize_track_column_width",
            start.elapsed(),
            Duration::from_millis(4),
            format!(
                "column={} rows={} width={width:.1}",
                Self::column_key(column),
                self.current_track_indices().len()
            ),
        );
        width
    }

    pub(super) fn table_column_text(
        &self,
        column: TableColumn,
        track_ix: usize,
        track: &Track,
    ) -> String {
        match column {
            TableColumn::Index => format!("{:02}", track_ix + 1),
            TableColumn::Artwork | TableColumn::Loved => String::new(),
            TableColumn::Title => track.title.clone(),
            TableColumn::Artist => track.artist.clone(),
            TableColumn::Album => track.album.clone(),
            TableColumn::TrackNumber => track
                .track_number
                .map(|track_number| track_number.to_string())
                .unwrap_or_default(),
            TableColumn::Format => track.codec.clone(),
            TableColumn::Bitrate => Self::bitrate_cell_label(track),
            TableColumn::FileSize => Self::file_size_label(track.file_size),
            TableColumn::Year => track.year.clone(),
            TableColumn::DateAdded => Self::date_label(track.date_added),
            TableColumn::Plays => track.plays.to_string(),
            TableColumn::Duration => track.duration.clone(),
        }
    }

    pub(super) fn resize_target_label(target: ColumnResizeTarget) -> &'static str {
        match target {
            ColumnResizeTarget::Track(column) => Self::column_label(column),
            ColumnResizeTarget::Artist(ArtistTableColumn::Artwork)
            | ColumnResizeTarget::Album(AlbumTableColumn::Artwork) => "",
            ColumnResizeTarget::Artist(ArtistTableColumn::Artist) => "Artist",
            ColumnResizeTarget::Artist(ArtistTableColumn::Albums) => "Albums",
            ColumnResizeTarget::Artist(ArtistTableColumn::Tracks) => "Tracks",
            ColumnResizeTarget::Album(AlbumTableColumn::Album) => "Album",
            ColumnResizeTarget::Album(AlbumTableColumn::Artist) => "Artist",
            ColumnResizeTarget::Album(AlbumTableColumn::Year) => "Year",
            ColumnResizeTarget::Album(AlbumTableColumn::Tracks) => "Tracks",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Index) => "#",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Path) => "PATH",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Error) => "ERROR",
            ColumnResizeTarget::PlaybackHistoryPlayedAt => "PLAYED AT",
        }
    }

    pub(super) fn resize_target_texts(&self, target: ColumnResizeTarget) -> Vec<String> {
        match target {
            ColumnResizeTarget::Artist(column) => self
                .artists
                .iter()
                .map(|artist| match column {
                    ArtistTableColumn::Artwork => String::new(),
                    ArtistTableColumn::Artist => artist.name.clone(),
                    ArtistTableColumn::Albums => artist.album_count.to_string(),
                    ArtistTableColumn::Tracks => artist.track_count.to_string(),
                })
                .collect(),
            ColumnResizeTarget::Album(column) => self
                .albums
                .iter()
                .map(|album| match column {
                    AlbumTableColumn::Artwork => String::new(),
                    AlbumTableColumn::Album => album.title.clone(),
                    AlbumTableColumn::Artist => album.artist.clone(),
                    AlbumTableColumn::Year => {
                        album.year.clone().unwrap_or_else(|| "Unknown".to_string())
                    }
                    AlbumTableColumn::Tracks => album.track_count.to_string(),
                })
                .collect(),
            ColumnResizeTarget::ScanError(column) => self
                .scan_errors
                .iter()
                .enumerate()
                .map(|(ix, error)| match column {
                    ScanErrorColumn::Index => (ix + 1).to_string(),
                    ScanErrorColumn::Path => error.path.display().to_string(),
                    ScanErrorColumn::Error => error.message.clone(),
                })
                .collect(),
            ColumnResizeTarget::PlaybackHistoryPlayedAt => self
                .playback_history
                .iter()
                .map(|entry| Self::history_played_at_label(entry.played_at_unix_secs))
                .collect(),
            ColumnResizeTarget::Track(_) => Vec::new(),
        }
    }

    pub(super) fn autosize_text_width(text: &str) -> f32 {
        text.chars()
            .map(|ch| match ch {
                'i' | 'j' | 'l' | 'I' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' => 0.45,
                'm' | 'w' | 'M' | 'W' | '@' | '#' | '%' | '&' => 1.35,
                'A'..='Z' => 1.1,
                _ => 1.0,
            })
            .sum::<f32>()
            * AUTOSIZE_TEXT_CHAR_W
    }

    pub(super) fn sanitize_visible_columns(columns: Vec<TableColumn>) -> Vec<TableColumn> {
        let mut sanitized = Vec::new();
        for column in columns {
            if ALL_TABLE_COLUMNS.contains(&column) && !sanitized.contains(&column) {
                sanitized.push(column);
            }
        }

        if !sanitized.contains(&TableColumn::Title) {
            sanitized.insert(0, TableColumn::Title);
        }
        if sanitized.is_empty() {
            default_visible_table_columns()
        } else {
            sanitized
        }
    }

    pub(super) fn show_column_menu(&mut self, event: &MouseDownEvent) {
        self.column_menu_open = true;
        self.column_menu_x = f32::from(event.position.x);
        self.column_menu_y = f32::from(event.position.y);
        self.context_menu_track = None;
    }

    pub(super) fn toggle_table_column(&mut self, column: TableColumn) {
        if column == TableColumn::Title {
            return;
        }

        if let Some(ix) = self
            .visible_columns
            .iter()
            .position(|visible| *visible == column)
        {
            self.visible_columns.remove(ix);
        } else if let Some(ix) = ALL_TABLE_COLUMNS
            .iter()
            .position(|available| *available == column)
        {
            let insert_ix = ALL_TABLE_COLUMNS[..ix]
                .iter()
                .filter(|available| self.visible_columns.contains(available))
                .count();
            self.visible_columns.insert(insert_ix, column);
        }

        self.visible_columns = Self::sanitize_visible_columns(self.visible_columns.clone());
        self.save_app_state();
    }

    pub(super) fn move_table_column_before(&mut self, moving: TableColumn, target: TableColumn) {
        if moving == target {
            return;
        }

        let Some(from_ix) = self
            .visible_columns
            .iter()
            .position(|column| *column == moving)
        else {
            return;
        };
        let Some(mut to_ix) = self
            .visible_columns
            .iter()
            .position(|column| *column == target)
        else {
            return;
        };

        let column = self.visible_columns.remove(from_ix);
        if from_ix < to_ix {
            to_ix = to_ix.saturating_sub(1);
        }
        self.visible_columns.insert(to_ix, column);
        self.save_app_state();
    }

    pub(super) fn handle_table_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        if event.keystroke.key.as_str() == "escape" {
            if self.cancel_table_scrollbar_drag()
                || self.cancel_table_horizontal_scrollbar_drag()
                || self.cancel_browse_scrollbar_drag()
            {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if self.page != Page::Library {
            return;
        }

        let handled = match event.keystroke.key.as_str() {
            "pageup" => {
                self.move_selection_by_page(-1);
                true
            }
            "pagedown" => {
                self.move_selection_by_page(1);
                true
            }
            "home" => {
                self.select_table_edge(false);
                true
            }
            "end" => {
                self.select_table_edge(true);
                true
            }
            _ => Self::album_jump_key(event)
                .map(|jump_key| self.scroll_to_album_initial(jump_key))
                .unwrap_or(false),
        };

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        self.move_selection_by_rows(delta, ScrollStrategy::Center);
    }

    pub(super) fn move_selection_by_page(&mut self, direction: isize) {
        let rows = self.table_page_row_count() as isize;
        self.move_selection_by_rows(direction * rows, ScrollStrategy::Center);
    }

    pub(super) fn move_selection_by_rows(&mut self, delta: isize, strategy: ScrollStrategy) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let selected_track = self.active_selected_track();
        let Some(position) = indices.iter().position(|ix| *ix == selected_track) else {
            return;
        };
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.select_table_row(next as usize, strategy);
    }

    pub(super) fn select_table_edge(&mut self, end: bool) {
        let Some(last_row) = self.current_track_indices().len().checked_sub(1) else {
            return;
        };

        if end {
            self.select_table_row(last_row, ScrollStrategy::Bottom);
        } else {
            self.select_table_row(0, ScrollStrategy::Top);
        }
    }

    pub(super) fn select_table_row(&mut self, row_ix: usize, strategy: ScrollStrategy) {
        let Some(track_ix) = self.current_track_indices().get(row_ix).copied() else {
            return;
        };

        self.set_active_selected_track(track_ix);
        self.active_tab()
            .table_scroll_handle
            .scroll_to_item(row_ix, strategy);
        self.context_menu_track = None;
    }

    pub(super) fn table_page_row_count(&self) -> usize {
        self.table_scrollbar_metrics()
            .map(|metrics| (metrics.track_height / TABLE_ROW_H).floor() as usize)
            .unwrap_or(12)
            .saturating_sub(1)
            .max(1)
    }

    pub(super) fn album_jump_key(event: &KeyDownEvent) -> Option<char> {
        let key_char = event.keystroke.key_char.as_deref()?;
        let mut chars = key_char.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        ch.is_ascii_alphanumeric().then(|| ch.to_ascii_uppercase())
    }

    pub(super) fn scroll_to_album_initial(&mut self, jump_key: char) -> bool {
        let Some(row_ix) = self.current_track_indices().iter().position(|track_ix| {
            self.tracks
                .get(*track_ix)
                .is_some_and(|track| Self::album_initial_matches(&track.album, jump_key))
        }) else {
            return false;
        };

        self.select_table_row(row_ix, ScrollStrategy::Top);
        true
    }

    pub(super) fn album_initial_matches(album: &str, jump_key: char) -> bool {
        album
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase() == jump_key)
            .unwrap_or(false)
    }

    pub(super) fn table_scrollbar_metrics(&self) -> Option<TableScrollbarMetrics> {
        let handle = self.active_tab().table_scroll_handle.clone();
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

    pub(super) fn table_horizontal_scrollbar_metrics(
        &self,
    ) -> Option<TableHorizontalScrollbarMetrics> {
        let handle = self.active_tab().table_scroll_handle.clone();
        let base_handle = handle.0.borrow().base_handle.clone();
        let bounds = base_handle.bounds();
        let viewport_width = self.table_horizontal_viewport_width()?;
        let content_width = self.table_content_width();
        let max_scroll = (content_width - viewport_width).max(0.0);
        if max_scroll <= 0.0 {
            return None;
        }

        let track_left = f32::from(bounds.origin.x) + 16.0;
        let track_width = (f32::from(bounds.size.width) - 16.0 - TABLE_SCROLLBAR_W).max(1.0);
        let thumb_width = ((viewport_width / content_width) * track_width)
            .max(TABLE_HORIZONTAL_SCROLLBAR_MIN_THUMB_W)
            .min(track_width);
        let thumb_travel = (track_width - thumb_width).max(0.0);
        let scroll_left = self.current_table_horizontal_scroll();
        let thumb_left = if max_scroll > 0.0 && thumb_travel > 0.0 {
            (scroll_left / max_scroll) * thumb_travel
        } else {
            0.0
        };

        Some(TableHorizontalScrollbarMetrics {
            track_left,
            track_width,
            thumb_left,
            thumb_width,
            max_scroll,
        })
    }

    pub(super) fn begin_table_scrollbar_drag(&mut self, event: &MouseDownEvent) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
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

        let start_offset = self.table_scrollbar_base_handle().offset();
        self.table_scrollbar_drag = Some(TableScrollbarDrag {
            thumb_offset,
            start_offset,
        });
        perf::event(
            "table.scrollbar.vertical.begin",
            format!("max_scroll={:.1}", metrics.max_scroll),
        );
        self.scroll_table_to_scrollbar_y(event.position.y, thumb_offset);
        true
    }

    pub(super) fn drag_table_scrollbar(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.table_scrollbar_drag else {
            return false;
        };

        if !event.dragging() {
            self.table_scrollbar_drag = None;
            return false;
        }

        self.scroll_table_to_scrollbar_y(event.position.y, drag.thumb_offset)
    }

    pub(super) fn finish_table_scrollbar_drag(&mut self) -> bool {
        let finished = self.table_scrollbar_drag.take().is_some();
        if finished {
            perf::event(
                "table.scrollbar.vertical.finish",
                format!(
                    "offset_y={:.1}",
                    f32::from(self.table_scrollbar_base_handle().offset().y)
                ),
            );
        }
        finished
    }

    pub(super) fn begin_table_horizontal_scrollbar_drag(&mut self, event: &MouseDownEvent) -> bool {
        let Some(metrics) = self.table_horizontal_scrollbar_metrics() else {
            return false;
        };

        let local_x = f32::from(event.position.x) - metrics.track_left;
        let thumb_right = metrics.thumb_left + metrics.thumb_width;
        let thumb_offset = if (metrics.thumb_left..=thumb_right).contains(&local_x) {
            local_x - metrics.thumb_left
        } else {
            metrics.thumb_width / 2.0
        };

        self.table_horizontal_scrollbar_drag = Some(TableHorizontalScrollbarDrag {
            thumb_offset,
            start_scroll: self.current_table_horizontal_scroll(),
        });
        perf::event(
            "table.scrollbar.horizontal.begin",
            format!(
                "offset={:.1} max_scroll={:.1}",
                self.current_table_horizontal_scroll(),
                metrics.max_scroll
            ),
        );
        self.scroll_table_to_horizontal_scrollbar_x(event.position.x, thumb_offset)
    }

    pub(super) fn drag_table_horizontal_scrollbar(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.table_horizontal_scrollbar_drag else {
            return false;
        };

        if !event.dragging() {
            self.table_horizontal_scrollbar_drag = None;
            return false;
        }

        self.scroll_table_to_horizontal_scrollbar_x(event.position.x, drag.thumb_offset)
    }

    pub(super) fn finish_table_horizontal_scrollbar_drag(&mut self) -> bool {
        let finished = self.table_horizontal_scrollbar_drag.take().is_some();
        if finished {
            perf::event(
                "table.scrollbar.horizontal.finish",
                format!("offset={:.1}", self.current_table_horizontal_scroll()),
            );
        }
        finished
    }

    pub(super) fn cancel_table_horizontal_scrollbar_drag(&mut self) -> bool {
        let Some(drag) = self.table_horizontal_scrollbar_drag.take() else {
            return false;
        };

        self.active_tab_mut().table_horizontal_scroll = drag.start_scroll;
        self.clamp_table_horizontal_scroll();
        true
    }

    pub(super) fn cancel_table_scrollbar_drag(&mut self) -> bool {
        let Some(drag) = self.table_scrollbar_drag.take() else {
            return false;
        };

        self.table_scrollbar_base_handle()
            .set_offset(drag.start_offset);
        true
    }

    pub(super) fn finish_table_drag_interactions(&mut self) -> bool {
        let scrolled = self.finish_table_scrollbar_drag();
        let horizontal_scrolled = self.finish_table_horizontal_scrollbar_drag();
        let resized = self.finish_column_resize();
        scrolled || horizontal_scrolled || resized
    }

    pub(super) fn mark_table_scrolling(&mut self, cx: &mut Context<Self>) {
        self.table_scroll_generation = self.table_scroll_generation.wrapping_add(1);

        if self.table_is_scrolling {
            return;
        }

        self.table_is_scrolling = true;
        perf::event(
            "table.scroll.begin",
            format!("rows={}", self.current_track_indices().len()),
        );
        cx.notify();

        cx.spawn(async move |this, cx| {
            loop {
                let Ok(generation) = this.update(cx, |app, _cx| app.table_scroll_generation) else {
                    return;
                };

                cx.background_executor()
                    .timer(TABLE_SCROLL_IDLE_DELAY)
                    .await;

                let Ok(should_stop) = this.update(cx, |app, cx| {
                    if app.table_scroll_generation == generation && app.table_is_scrolling {
                        app.table_is_scrolling = false;
                        perf::event("table.scroll.end", format!("generation={generation}"));
                        cx.notify();
                        true
                    } else {
                        false
                    }
                }) else {
                    return;
                };

                if should_stop {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn scroll_table_to_scrollbar_y(
        &mut self,
        mouse_y: Pixels,
        thumb_offset: f32,
    ) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let thumb_travel = (metrics.track_height - metrics.thumb_height).max(1.0);
        let thumb_top =
            (f32::from(mouse_y) - metrics.track_top - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_top / thumb_travel;
        self.scroll_table_to_ratio(ratio)
    }

    pub(super) fn scroll_table_to_ratio(&mut self, ratio: f32) -> bool {
        let base_handle = self.table_scrollbar_base_handle();
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

    pub(super) fn scroll_table_to_horizontal_scrollbar_x(
        &mut self,
        mouse_x: Pixels,
        thumb_offset: f32,
    ) -> bool {
        let Some(metrics) = self.table_horizontal_scrollbar_metrics() else {
            return false;
        };

        let thumb_travel = (metrics.track_width - metrics.thumb_width).max(1.0);
        let thumb_left =
            (f32::from(mouse_x) - metrics.track_left - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_left / thumb_travel;
        let next = ratio.clamp(0.0, 1.0) * metrics.max_scroll;
        if (next - self.current_table_horizontal_scroll()).abs() < 0.5 {
            return false;
        }

        self.active_tab_mut().table_horizontal_scroll = next;
        true
    }

    pub(super) fn table_scrollbar_base_handle(&self) -> gpui::ScrollHandle {
        self.active_tab()
            .table_scroll_handle
            .0
            .borrow()
            .base_handle
            .clone()
    }

    pub(super) fn restore_active_table_scroll_position(&mut self) {
        let Some(scroll_top) = self.active_tab_mut().restore_table_scroll_top.take() else {
            return;
        };

        let base_handle = self.table_scrollbar_base_handle();
        let current = base_handle.offset();
        base_handle.set_offset(point(current.x, px(-scroll_top.max(0.0))));
    }

    pub(super) fn current_scrollbar_label(&self, metrics: TableScrollbarMetrics) -> Option<String> {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return None;
        }

        let ratio = if metrics.max_scroll > 0.0 {
            metrics.scroll_top / metrics.max_scroll
        } else {
            0.0
        };
        let row_ix =
            (ratio.clamp(0.0, 1.0) * indices.len().saturating_sub(1) as f32).round() as usize;
        let track_ix = *indices.get(row_ix)?;
        Some(self.scrollbar_marker_label(track_ix, self.active_tab().sort_column))
    }

    pub(super) fn fast_scroll_top_row(&self) -> (usize, f32) {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return (0, 0.0);
        };

        let scroll_top = metrics.scroll_top.max(0.0);
        let row = (scroll_top / TABLE_ROW_H).floor() as usize;
        let offset = -(scroll_top % TABLE_ROW_H);
        (row, offset)
    }

    pub(super) fn render_table(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.restore_active_table_scroll_position();
        let item_count = self.current_track_indices().len();
        let active_search_query = self.active_search_query().to_string();
        let has_no_search_results = item_count == 0
            && self.active_source_track_count() > 0
            && !active_search_query.trim().is_empty();
        let table_scroll_handle = self.active_tab().table_scroll_handle.clone();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let scrolled = this.drag_table_scrollbar(event);
                let horizontal_scrolled = !scrolled && this.drag_table_horizontal_scrollbar(event);
                let resized =
                    !scrolled && !horizontal_scrolled && this.resize_column_from_mouse(event);
                if scrolled || horizontal_scrolled || resized {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_table_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(self.table_scrollbar_drag.is_some(), |this| {
                        this.child(self.render_fast_scroll_rows())
                    })
                    .when(self.table_scrollbar_drag.is_none(), |this| {
                        this.child(
                            uniform_list(
                                "track-table-rows",
                                item_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    range
                                        .enumerate()
                                        .filter_map(|(visible_row_ix, row_ix)| {
                                            let track_ix = this
                                                .current_track_indices()
                                                .get(row_ix)
                                                .copied()?;
                                            Some(
                                                this.render_track_row(
                                                    visible_row_ix,
                                                    track_ix,
                                                    &this.tracks[track_ix],
                                                    this.table_is_scrolling,
                                                    cx,
                                                )
                                                .into_any_element(),
                                            )
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, _window, cx| {
                                    this.handle_table_scroll_wheel(event, cx);
                                },
                            ))
                            .track_scroll(table_scroll_handle),
                        )
                    })
                    .child(self.render_table_scrollbar(item_count, cx))
                    .child(self.render_table_horizontal_scrollbar(cx)),
            )
            .when(self.tracks.is_empty(), |this| {
                let colors = *self.colors();
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
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
                                .child("No indexed audio yet"),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.text_muted))
                                .child(self.library_status.clone()),
                        ),
                )
            })
            .when(has_no_search_results, |this| {
                let colors = *self.colors();
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
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
                                .child("No matching tracks"),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.text_muted))
                                .child(format!("No tracks match \"{}\".", active_search_query)),
                        ),
                )
            })
    }

    pub(super) fn render_fast_scroll_rows(&self) -> AnyElement {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return div().size_full().into_any_element();
        };

        let indices = self.current_track_indices();
        if indices.is_empty() {
            return div().size_full().into_any_element();
        }

        let viewport_height = metrics.track_height + TABLE_SCROLLBAR_MARGIN * 2.0;
        let visible_rows = (viewport_height / TABLE_ROW_H).ceil() as usize + 2;
        let (top_row, row_offset) = self.fast_scroll_top_row();
        let start_row = top_row.saturating_sub(FAST_SCROLL_OVERSCAN_ROWS);
        let rows_to_render = visible_rows + FAST_SCROLL_OVERSCAN_ROWS * 2;
        let first_row_top = row_offset - ((top_row - start_row) as f32 * TABLE_ROW_H);
        let rows = (0..rows_to_render)
            .filter_map(|visible_ix| {
                let row_ix = start_row + visible_ix;
                let track_ix = *indices.get(row_ix)?;
                let top = first_row_top + visible_ix as f32 * TABLE_ROW_H;
                Some(self.render_fast_track_row(top, track_ix, &self.tracks[track_ix]))
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .overflow_hidden()
            .bg(rgb(self.colors().surface))
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_fast_track_row(
        &self,
        top: f32,
        track_ix: usize,
        track: &Track,
    ) -> AnyElement {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let colors = *self.colors();
        let bg = if selected {
            colors.selected
        } else if active {
            colors.playing
        } else {
            colors.row
        };
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(bg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .ml(px(-self.current_table_horizontal_scroll()))
                    .w(px(self.table_render_width()))
                    .children(
                        self.visible_columns
                            .iter()
                            .copied()
                            .map(|column| self.track_cell(column, track_ix, track, active, true)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_table_scrollbar(
        &self,
        item_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item_count == 0 {
            return div().into_any_element();
        }
        let colors = *self.colors();

        let metrics = self.table_scrollbar_metrics();
        let thumb_top = metrics.map_or(0.0, |metrics| metrics.thumb_top);
        let thumb_height =
            metrics.map_or(TABLE_SCROLLBAR_MIN_THUMB_H, |metrics| metrics.thumb_height);
        let track_height = metrics.map_or(0.0, |metrics| metrics.track_height);
        let scrollable = metrics.is_some_and(|metrics| metrics.max_scroll > 0.0);
        let current_label = metrics.and_then(|metrics| self.current_scrollbar_label(metrics));
        let max_markers = if track_height > 0.0 {
            ((track_height / 16.0).floor() as usize).clamp(2, TABLE_SCROLLBAR_MAX_MARKERS)
        } else {
            0
        };
        let marker_stride = self
            .active_tab()
            .scrollbar_markers
            .len()
            .saturating_add(max_markers.saturating_sub(1))
            / max_markers.max(1);
        let marker_stride = marker_stride.max(1);
        let markers = self
            .active_tab()
            .scrollbar_markers
            .iter()
            .enumerate()
            .filter_map(|(ix, marker)| {
                if ix % marker_stride != 0 {
                    return None;
                }

                let top = TABLE_SCROLLBAR_MARGIN + marker.ratio.clamp(0.0, 1.0) * track_height;
                Some(
                    div()
                        .absolute()
                        .top(px((top - 7.0).max(TABLE_SCROLLBAR_MARGIN)))
                        .right(px(14.0))
                        .w(px(30.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(rgb(colors.text_faint))
                        .child(marker.label.clone())
                        .into_any_element(),
                )
            })
            .collect::<Vec<_>>();
        let current_label_top = (TABLE_SCROLLBAR_MARGIN + thumb_top + thumb_height / 2.0 - 9.0)
            .clamp(
                TABLE_SCROLLBAR_MARGIN,
                (track_height - 18.0).max(TABLE_SCROLLBAR_MARGIN),
            );
        let hints_opacity = if self.table_scrollbar_drag.is_some() {
            1.0
        } else {
            0.0
        };

        div()
            .id("table-scrollbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(TABLE_SCROLLBAR_W))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if this.begin_table_scrollbar_drag(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_table_scrollbar(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(TABLE_SCROLLBAR_W))
                    .opacity(hints_opacity)
                    .hover(|this| this.opacity(1.0))
                    .children(markers)
                    .when(scrollable, |this| {
                        this.when_some(current_label, |this, label| {
                            this.child(
                                div()
                                    .absolute()
                                    .top(px(current_label_top))
                                    .right(px(15.0))
                                    .max_w(px(36.0))
                                    .h(px(18.0))
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(colors.playing))
                                    .border_1()
                                    .border_color(rgb(colors.border_strong))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(rgb(colors.text))
                                    .overflow_hidden()
                                    .child(label),
                            )
                        })
                    }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(TABLE_SCROLLBAR_MARGIN))
                    .right(px(4.0))
                    .bottom(px(TABLE_SCROLLBAR_MARGIN))
                    .w(px(TABLE_SCROLLBAR_TRACK_W))
                    .rounded_full()
                    .bg(rgb(colors.elevated))
                    .opacity(if scrollable { 0.95 } else { 0.45 })
                    .child(
                        div()
                            .absolute()
                            .top(px(thumb_top))
                            .left(px(1.0))
                            .right(px(1.0))
                            .h(px(thumb_height))
                            .rounded_full()
                            .bg(rgb(if self.table_scrollbar_drag.is_some() {
                                colors.text
                            } else {
                                colors.text_faint
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_table_horizontal_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.table_horizontal_scrollbar_metrics() else {
            return div().into_any_element();
        };

        let colors = *self.colors();
        div()
            .id("table-horizontal-scrollbar")
            .absolute()
            .left(px(16.0))
            .right(px(TABLE_SCROLLBAR_W))
            .bottom_0()
            .h(px(TABLE_HORIZONTAL_SCROLLBAR_H))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if this.begin_table_horizontal_scrollbar_drag(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_table_horizontal_scrollbar(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_horizontal_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(TABLE_HORIZONTAL_SCROLLBAR_MARGIN))
                    .h(px(TABLE_HORIZONTAL_SCROLLBAR_TRACK_H))
                    .rounded_full()
                    .bg(rgb(colors.elevated))
                    .opacity(0.95)
                    .child(
                        div()
                            .absolute()
                            .left(px(metrics.thumb_left))
                            .top(px(1.0))
                            .bottom(px(1.0))
                            .w(px(metrics.thumb_width))
                            .rounded_full()
                            .bg(rgb(if self.table_horizontal_scrollbar_drag.is_some() {
                                colors.text
                            } else {
                                colors.text_faint
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_table_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .id("table-header")
            .h(px(27.0))
            .px_4()
            .flex()
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(colors.text_faint))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.show_column_menu(event);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .ml(px(-self.current_table_horizontal_scroll()))
                    .w(px(self.table_render_width()))
                    .children(
                        self.visible_columns
                            .iter()
                            .copied()
                            .map(|column| self.header_cell(column, cx)),
                    ),
            )
    }

    pub(super) fn render_resizable_table_header(
        &self,
        height: f32,
        columns: &[ColumnResizeTarget],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        div()
            .h(px(height))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(colors.text_faint))
            .children(
                columns
                    .iter()
                    .copied()
                    .map(|target| self.resizable_header_cell(target, cx)),
            )
            .into_any_element()
    }

    pub(super) fn header_cell(
        &self,
        column: TableColumn,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let label = Self::column_label(column);
        let sort_column = Self::sort_column_for(column);
        let width = self.table_column_render_width(column);
        let tab = self.active_tab();
        let active = Self::header_sort_active(column, tab.sort_column);
        let colors = *self.colors();
        let icon = Self::header_sort_icon(column, tab.sort_column, tab.sort_direction);
        let id = SharedString::from(format!("column-{}", Self::column_key(column)));

        div()
            .id(id)
            .relative()
            .h_full()
            .w(px(width))
            .flex()
            .items_center()
            .gap_1()
            .text_color(rgb(if active {
                colors.text
            } else {
                colors.text_faint
            }))
            .when(sort_column.is_some(), |this| {
                this.cursor_pointer()
                    .hover(move |this| this.text_color(rgb(colors.text)))
            })
            .child(label)
            .when(active, |this| this.child(icon))
            .when_some(sort_column, |this, sort_column| {
                this.on_click(cx.listener(move |this, _, _, cx| {
                    let sort_start = Instant::now();
                    let tab = this.active_tab_mut();
                    if column == TableColumn::Album {
                        (tab.sort_column, tab.sort_direction) =
                            Self::next_album_sort(tab.sort_column, tab.sort_direction);
                    } else if tab.sort_column == sort_column {
                        tab.sort_direction = match tab.sort_direction {
                            SortDirection::Ascending => SortDirection::Descending,
                            SortDirection::Descending => SortDirection::Ascending,
                        };
                    } else {
                        tab.sort_column = sort_column;
                        tab.sort_direction = SortDirection::Ascending;
                    }

                    this.invalidate_track_indices();
                    perf::log_duration(
                        "table.sort_header_click",
                        sort_start.elapsed(),
                        format!(
                            "column={} results={}",
                            Self::column_key(column),
                            this.current_track_indices().len()
                        ),
                    );
                    cx.notify();
                }))
            })
            .on_drag(
                ColumnDrag::new(column, label),
                |drag: &ColumnDrag, position, _, cx| {
                    let preview = drag.clone().position(position);
                    cx.new(|_| preview)
                },
            )
            .on_drop(cx.listener(move |this, drag: &ColumnDrag, _window, cx| {
                this.move_table_column_before(drag.column, column);
                cx.notify();
            }))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "column-resizer-{}",
                        Self::column_key(column)
                    )))
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(CursorStyle::ResizeColumn)
                    .hover(move |this| this.bg(rgb(colors.border_strong)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.begin_column_resize(column, event);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                        if event.standard_click() && event.click_count() >= 2 {
                            this.autosize_table_column(column);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    })),
            )
    }

    pub(super) fn resizable_header_cell(
        &self,
        target: ColumnResizeTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = Self::resize_target_label(target);
        let width = self.resize_target_width(target);
        let colors = *self.colors();
        let id = SharedString::from(format!("header-{}", Self::resize_target_key(target)));

        div()
            .id(id)
            .relative()
            .h_full()
            .w(px(width))
            .flex()
            .items_center()
            .text_color(rgb(colors.text_faint))
            .child(label)
            .child(
                div()
                    .id(SharedString::from(format!(
                        "header-resizer-{}",
                        Self::resize_target_key(target)
                    )))
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(CursorStyle::ResizeColumn)
                    .hover(move |this| this.bg(rgb(colors.border_strong)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.begin_resize_target(target, event);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                        if event.standard_click() && event.click_count() >= 2 {
                            this.autosize_resize_target(target);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_track_row(
        &self,
        _row_ix: usize,
        track_ix: usize,
        track: &Track,
        lightweight: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let colors = *self.colors();
        let bg = if selected {
            colors.selected
        } else if active {
            colors.playing
        } else {
            colors.row
        };
        div()
            .id(SharedString::from(format!("track-row-{track_ix}")))
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(bg))
            .when(!lightweight, |this| {
                this.cursor_pointer()
                    .hover(move |this| this.bg(rgb(colors.hover)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        window.focus(&this.focus_handle);
                        this.set_active_selected_track(track_ix);
                        this.context_menu_track = None;
                        this.column_menu_open = false;

                        if event.standard_click() && event.modifiers().control {
                            this.queue_track(track_ix);
                            cx.notify();
                            return;
                        }

                        if event.standard_click() && event.click_count() >= 2 {
                            this.play_track(track_ix);
                        }

                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle);
                            this.set_active_selected_track(track_ix);
                            this.context_menu_track = Some(track_ix);
                            this.column_menu_open = false;
                            this.context_menu_position = event.position;
                            cx.notify();
                        }),
                    )
                    .on_drag(
                        TrackDrag::new(track_ix, track),
                        |drag: &TrackDrag, position, _, cx| {
                            let preview = drag.clone().position(position);
                            cx.new(|_| preview)
                        },
                    )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .ml(px(-self.current_table_horizontal_scroll()))
                    .w(px(self.table_render_width()))
                    .children(self.visible_columns.iter().copied().map(|column| {
                        self.track_cell(column, track_ix, track, active, lightweight)
                    })),
            )
    }

    pub(super) fn cell(&self, content: impl Into<SharedString>, width: f32) -> impl IntoElement {
        div()
            .w(px(width))
            .overflow_hidden()
            .text_ellipsis()
            .text_color(rgb(self.colors().text_muted))
            .child(content.into())
    }

    pub(super) fn track_cell(
        &self,
        column: TableColumn,
        track_ix: usize,
        track: &Track,
        active: bool,
        lightweight: bool,
    ) -> AnyElement {
        let colors = *self.colors();
        let width = self.table_column_render_width(column);
        match column {
            TableColumn::Index => div()
                .w(px(width))
                .text_xs()
                .text_color(rgb(colors.text_faint))
                .child(if active {
                    if self.is_playing { "Ⅱ" } else { "▶" }.into()
                } else {
                    format!("{:02}", track_ix + 1)
                })
                .into_any_element(),
            TableColumn::Artwork => div()
                .w(px(width))
                .flex()
                .items_center()
                .child(if lightweight {
                    self.album_tile_placeholder(track, 22.0)
                } else {
                    self.album_tile(track, 22.0)
                })
                .into_any_element(),
            TableColumn::Title => div()
                .w(px(width))
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(if active {
                    colors.accent
                } else {
                    colors.text_strong
                }))
                .child(track.title.clone())
                .into_any_element(),
            TableColumn::Artist => self.cell(track.artist.clone(), width).into_any_element(),
            TableColumn::Album => self.cell(track.album.clone(), width).into_any_element(),
            TableColumn::TrackNumber => self
                .cell(
                    track
                        .track_number
                        .map(|track_number| track_number.to_string())
                        .unwrap_or_default(),
                    width,
                )
                .into_any_element(),
            TableColumn::Format => self.cell(track.codec.clone(), width).into_any_element(),
            TableColumn::Bitrate => self
                .cell(Self::bitrate_cell_label(track), width)
                .into_any_element(),
            TableColumn::FileSize => self
                .cell(Self::file_size_label(track.file_size), width)
                .into_any_element(),
            TableColumn::Year => self.cell(track.year.clone(), width).into_any_element(),
            TableColumn::DateAdded => self
                .cell(Self::date_label(track.date_added), width)
                .into_any_element(),
            TableColumn::Plays => self.cell(track.plays.to_string(), width).into_any_element(),
            TableColumn::Duration => self.cell(track.duration.clone(), width).into_any_element(),
            TableColumn::Loved => div()
                .w(px(width))
                .text_color(rgb(colors.love))
                .child(if track.loved { "♥" } else { "" })
                .into_any_element(),
        }
    }

    pub(super) fn render_column_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        self.menu_at(
            point(px(self.column_menu_x), px(self.column_menu_y)),
            Corner::TopLeft,
            point(px(2.0), px(2.0)),
            self.menu_panel(220.0)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(self.menu_header("Columns"))
                .children(
                    ALL_TABLE_COLUMNS
                        .iter()
                        .copied()
                        .map(|column| self.column_menu_item(column, cx)),
                ),
        )
    }

    pub(super) fn column_menu_item(
        &self,
        column: TableColumn,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let checked = self.visible_columns.contains(&column);
        let locked = column == TableColumn::Title;
        self.menu_item_base(SharedString::from(format!(
            "column-menu-{}",
            Self::column_key(column)
        )))
        .gap_2()
        .cursor(if locked {
            CursorStyle::Arrow
        } else {
            CursorStyle::PointingHand
        })
        .text_color(rgb(if locked {
            colors.text_faint
        } else {
            colors.text
        }))
        .child(
            div()
                .w(px(16.0))
                .text_color(rgb(colors.accent))
                .child(if checked { "✓" } else { "" }),
        )
        .child(div().flex_1().child(Self::column_menu_label(column)))
        .when(locked, |this| this.child(div().text_xs().child("required")))
        .when(!locked, |this| {
            this.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.toggle_table_column(column);
                cx.stop_propagation();
                cx.notify();
            }))
        })
    }

    pub(super) fn column_label(column: TableColumn) -> &'static str {
        match column {
            TableColumn::Index => "#",
            TableColumn::Artwork => "",
            TableColumn::Title => "TITLE",
            TableColumn::Artist => "ARTIST",
            TableColumn::Album => "ALBUM",
            TableColumn::TrackNumber => "TRK",
            TableColumn::Format => "FMT",
            TableColumn::Bitrate => "BITRATE",
            TableColumn::FileSize => "SIZE",
            TableColumn::Year => "YEAR",
            TableColumn::DateAdded => "ADDED",
            TableColumn::Plays => "PLAYS",
            TableColumn::Duration => "TIME",
            TableColumn::Loved => "",
        }
    }

    pub(super) fn column_menu_label(column: TableColumn) -> &'static str {
        match column {
            TableColumn::Artwork => "Artwork",
            TableColumn::Loved => "Loved",
            _ => Self::column_label(column),
        }
    }

    pub(super) fn column_key(column: TableColumn) -> &'static str {
        match column {
            TableColumn::Index => "index",
            TableColumn::Artwork => "artwork",
            TableColumn::Title => "title",
            TableColumn::Artist => "artist",
            TableColumn::Album => "album",
            TableColumn::TrackNumber => "track-number",
            TableColumn::Format => "format",
            TableColumn::Bitrate => "bitrate",
            TableColumn::FileSize => "file-size",
            TableColumn::Year => "year",
            TableColumn::DateAdded => "date-added",
            TableColumn::Plays => "plays",
            TableColumn::Duration => "duration",
            TableColumn::Loved => "loved",
        }
    }

    pub(super) fn resize_target_key(target: ColumnResizeTarget) -> &'static str {
        match target {
            ColumnResizeTarget::Track(column) => Self::column_key(column),
            ColumnResizeTarget::Artist(ArtistTableColumn::Artwork) => "artist-artwork",
            ColumnResizeTarget::Artist(ArtistTableColumn::Artist) => "artist-name",
            ColumnResizeTarget::Artist(ArtistTableColumn::Albums) => "artist-albums",
            ColumnResizeTarget::Artist(ArtistTableColumn::Tracks) => "artist-tracks",
            ColumnResizeTarget::Album(AlbumTableColumn::Artwork) => "album-artwork",
            ColumnResizeTarget::Album(AlbumTableColumn::Album) => "album-title",
            ColumnResizeTarget::Album(AlbumTableColumn::Artist) => "album-artist",
            ColumnResizeTarget::Album(AlbumTableColumn::Year) => "album-year",
            ColumnResizeTarget::Album(AlbumTableColumn::Tracks) => "album-tracks",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Index) => "scan-error-index",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Path) => "scan-error-path",
            ColumnResizeTarget::ScanError(ScanErrorColumn::Error) => "scan-error-error",
            ColumnResizeTarget::PlaybackHistoryPlayedAt => "playback-history-played-at",
        }
    }

    pub(super) fn sort_column_for(column: TableColumn) -> Option<SortColumn> {
        match column {
            TableColumn::Index => Some(SortColumn::Index),
            TableColumn::Title => Some(SortColumn::Title),
            TableColumn::Artist => Some(SortColumn::Artist),
            TableColumn::Album => Some(SortColumn::Album),
            TableColumn::TrackNumber => Some(SortColumn::TrackNumber),
            TableColumn::Format => Some(SortColumn::Format),
            TableColumn::Bitrate => Some(SortColumn::Bitrate),
            TableColumn::FileSize => Some(SortColumn::FileSize),
            TableColumn::Year => Some(SortColumn::Year),
            TableColumn::DateAdded => Some(SortColumn::DateAdded),
            TableColumn::Plays => Some(SortColumn::Plays),
            TableColumn::Duration => Some(SortColumn::Duration),
            TableColumn::Artwork | TableColumn::Loved => None,
        }
    }

    pub(super) fn header_sort_active(column: TableColumn, sort_column: SortColumn) -> bool {
        match column {
            TableColumn::Album => {
                matches!(sort_column, SortColumn::Album | SortColumn::AlbumByArtist)
            }
            _ => Self::sort_column_for(column).is_some_and(|column| sort_column == column),
        }
    }

    pub(super) fn header_sort_icon(
        column: TableColumn,
        sort_column: SortColumn,
        sort_direction: SortDirection,
    ) -> &'static str {
        match (column, sort_column, sort_direction) {
            (TableColumn::Album, SortColumn::AlbumByArtist, SortDirection::Ascending) => "▲ artist",
            (TableColumn::Album, SortColumn::Album, SortDirection::Ascending) => "▲ A-Z",
            (TableColumn::Album, SortColumn::AlbumByArtist, SortDirection::Descending) => {
                "▼ artist"
            }
            (TableColumn::Album, SortColumn::Album, SortDirection::Descending) => "▼ Z-A",
            (_, _, SortDirection::Ascending) => "▲",
            (_, _, SortDirection::Descending) => "▼",
        }
    }

    pub(super) fn next_album_sort(
        sort_column: SortColumn,
        sort_direction: SortDirection,
    ) -> (SortColumn, SortDirection) {
        match (sort_column, sort_direction) {
            (SortColumn::AlbumByArtist, SortDirection::Ascending) => {
                (SortColumn::Album, SortDirection::Ascending)
            }
            (SortColumn::Album, SortDirection::Ascending) => {
                (SortColumn::AlbumByArtist, SortDirection::Descending)
            }
            (SortColumn::AlbumByArtist, SortDirection::Descending) => {
                (SortColumn::Album, SortDirection::Descending)
            }
            (SortColumn::Album, SortDirection::Descending) => {
                (SortColumn::AlbumByArtist, SortDirection::Ascending)
            }
            _ => (SortColumn::AlbumByArtist, SortDirection::Ascending),
        }
    }

    pub(super) fn bitrate_cell_label(track: &Track) -> String {
        track
            .bitrate
            .map(|bitrate| format!("{bitrate} kbps"))
            .unwrap_or_default()
    }

    pub(super) fn file_size_label(bytes: u64) -> String {
        if bytes >= 1_000_000_000 {
            format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
        } else if bytes >= 1_000_000 {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        } else {
            format!("{} KB", bytes / 1_000)
        }
    }

    pub(super) fn date_label(time: SystemTime) -> String {
        let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
            return String::new();
        };
        let days = (duration.as_secs() / 86_400) as i64;
        let (year, month, day) = Self::civil_date_from_days(days);
        format!("{year:04}-{month:02}-{day:02}")
    }

    pub(super) fn date_time_label_from_unix(unix_secs: u64) -> String {
        let days = (unix_secs / 86_400) as i64;
        let seconds = unix_secs % 86_400;
        let (year, month, day) = Self::civil_date_from_days(days);
        let hour = seconds / 3_600;
        let minute = (seconds % 3_600) / 60;
        let second = seconds % 60;

        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
    }

    pub(super) fn civil_date_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
        let z = days_since_epoch + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        if month <= 2 {
            year += 1;
        }
        (year, month, day)
    }

    pub(super) fn render_context_menu(
        &self,
        track_ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let track = &self.tracks[track_ix];
        self.menu_at(
            self.context_menu_position,
            Corner::TopLeft,
            point(px(2.0), px(2.0)),
            self.menu_panel(190.0)
                .child(self.menu_header(track.title.clone()))
                .child(
                    self.context_menu_item("Play from start")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if track_ix < this.tracks.len() {
                                this.play_track(track_ix);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    self.context_menu_item("Add to start of queue")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.queue_track_at_start(track_ix);
                            cx.notify();
                        })),
                )
                .child(
                    self.context_menu_item("Add to end of queue")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.queue_track_at_end(track_ix);
                            cx.notify();
                        })),
                )
                .child(self.context_menu_item("Queue Album").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.queue_album_from_track(track_ix, false);
                        cx.notify();
                    },
                )))
                .child(
                    self.context_menu_item("Queue Album Shuffled")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.queue_album_from_track(track_ix, true);
                            cx.notify();
                        })),
                )
                .when(!self.playlists.is_empty(), |this| {
                    this.child(self.menu_section_label("ADD TO PLAYLIST"))
                        .children(self.playlists.iter().enumerate().map(
                            |(playlist_ix, playlist)| {
                                self.context_menu_item_dynamic(format!("Add to {}", playlist.name))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.add_track_to_playlist(track_ix, playlist_ix);
                                        cx.notify();
                                    }))
                            },
                        ))
                })
                .child(self.context_menu_item("Go to album"))
                .child(self.context_menu_item("Show file")),
        )
    }

    pub(super) fn context_menu_item(&self, label: &'static str) -> gpui::Stateful<gpui::Div> {
        self.menu_item(SharedString::from(format!("context-menu-{label}")), label)
    }

    pub(super) fn context_menu_item_dynamic(&self, label: String) -> gpui::Stateful<gpui::Div> {
        let id = SharedString::from(format!("context-menu-{label}"));
        self.menu_item(id, label)
    }
}
