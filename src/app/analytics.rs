//! Analytics dashboard. Aggregates the in-memory `tracks` and
//! `playback_history` into a set of pure data structures, then renders
//! them through the reusable visualization components in
//! [`super::charts`].
//!
//! Two concerns deliberately stay separate:
//!
//! - **Library stats** (genre, codec, decades, file types, size
//!   buckets, sample rate, …) describe the *whole* indexed library
//!   and ignore the user's time-range filter.
//! - **Listening stats** (hours played, top artists, hour-of-day,
//!   weekday/hour grid, recent activity, gathering dust, streak, …)
//!   are computed from playback history filtered by
//!   `TempoApp::analytics_time_range`.
//!
//! The right-side filter sidebar is a separate render path that
//! mutates `TempoApp::analytics_time_range` and `analytics_sidebar_collapsed`
//! and re-paints. All chart panels are produced by the reusable
//! components in `super::charts` so adding new analytics elsewhere
//! later is mostly a matter of new aggregations + a few component
//! calls.

use std::collections::{BTreeMap, HashMap};

use chrono::{
    DateTime, Datelike as _, Duration as ChronoDuration, Local, NaiveDate, TimeZone, Timelike as _,
};
use gpui::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px, rgb,
};

use super::{
    AnalyticsTimeRange, RIGHT_SIDEBAR_W, TempoApp, Track, TrackArtwork,
    charts::{
        ChartCtx, DonutSlice, HBarRow, HeatmapCell, LegendItem, RankedRow, RowImage,
        RowImageSource, StackSegment, VBar, blend_color, donut_svg, h_bar_list, heatmap,
        legend_grid, panel, radial_hours, ranked_list, section_header, sparkline, stacked_bar,
        vertical_bar_chart,
    },
};

const ANALYTICS_HEATMAP_DAYS: usize = 365;

// ============================================================================
// Local helpers for resolving inline imagery in the analytics dashboard.
// ============================================================================

impl TempoApp {
    /// Locate the in-memory `Track` for a given path. Used by the
    /// analytics dashboard to surface album artwork next to the
    /// "TOP TRACKS" / "GATHERING DUST" rows. The library can be tens
    /// of thousands of tracks long, so callers should batch lookups
    /// (the analytics page only does 8–10 per render).
    pub(super) fn find_track_for_path(&self, path: &std::path::Path) -> Option<&Track> {
        self.tracks.iter().find(|track| track.path == path)
    }

    /// Build a `RowImage` for the given artist name. Returns `None`
    /// when no matching `Artist` is in memory (which happens for
    /// playback history rows whose underlying track is no longer
    /// indexed); the caller should still emit a `RowImage` with a
    /// fallback initials/color tile when displaying such rows.
    pub(super) fn analytics_row_image_for_artist(&self, name: &str) -> RowImage {
        if let Some(artist) = self.artists.iter().find(|a| a.name == name) {
            return RowImage {
                source: artist
                    .photo_path
                    .as_ref()
                    .map(|p| RowImageSource::File(p.clone())),
                initials: SharedString::from(artist.initials.clone()),
                color: artist.color,
                round: true,
            };
        }
        RowImage {
            source: None,
            initials: SharedString::from(super::artwork::initials_for(name)),
            color: super::artwork::color_for(name, name),
            round: true,
        }
    }

    /// Build a `RowImage` for the given album. Looks up the indexed
    /// `Album` first; falls back to the supplied sample `Track` (when
    /// an `Album` row hasn't been built yet, e.g. for synthesized
    /// detail tabs) and finally to a deterministic placeholder.
    pub(super) fn analytics_row_image_for_album(
        &self,
        title: &str,
        artist: &str,
        sample_track_path: Option<&std::path::Path>,
    ) -> RowImage {
        if let Some(album) = self
            .albums
            .iter()
            .find(|a| a.title == title && a.artist == artist)
        {
            return RowImage {
                source: album
                    .artwork_path
                    .as_ref()
                    .map(|p| RowImageSource::File(p.clone())),
                initials: SharedString::from(album.initials.clone()),
                color: album.color,
                round: false,
            };
        }
        if let Some(track) = sample_track_path.and_then(|p| self.find_track_for_path(p)) {
            return row_image_for_track(track);
        }
        RowImage {
            source: None,
            initials: SharedString::from(super::artwork::album_initials_for(title, title)),
            color: super::artwork::album_color_for(title, artist),
            round: false,
        }
    }
}

/// Build a `RowImage` directly from a `Track`, using whichever
/// artwork variant the catalog produced (embedded bytes vs.
/// extracted file).
fn row_image_for_track(track: &Track) -> RowImage {
    let source = match &track.artwork {
        Some(TrackArtwork::Embedded(image)) => Some(RowImageSource::Embedded(image.clone())),
        Some(TrackArtwork::File(path)) => Some(RowImageSource::File(path.clone())),
        None => None,
    };
    RowImage {
        source,
        initials: SharedString::from(track.album_initials.clone()),
        color: track.album_color,
        round: false,
    }
}

// ============================================================================
// Public API
// ============================================================================

impl TempoApp {
    /// Render the Analytics page. Lays out a series of cards and
    /// visualizations describing the current library and listening
    /// behavior.
    pub(super) fn render_analytics_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let summary = compute_summary(
            &self.tracks,
            &self.playback_history,
            self.analytics_time_range,
        );
        let subtitle = format!(
            "{} tracks · {} artists · {} albums · {} · {}",
            Self::format_count_short(self.tracks.len()),
            Self::format_count_short(self.artists.len()),
            Self::format_count_short(self.albums.len()),
            Self::format_library_size_bytes(self.library_size_bytes),
            self.analytics_time_range.long_label(),
        );

        div()
            .id("analytics-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_simple_page_header("Analytics", subtitle))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(self.render_analytics_body(summary, cx))
            .into_any_element()
    }

    fn render_analytics_body(
        &self,
        summary: AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        if self.tracks.is_empty() {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .max_w(px(420.0))
                        .p_5()
                        .text_color(rgb(colors.text_muted))
                        .child(
                            "No tracks indexed yet. Add a music folder in Settings and \
                             Tempo will populate analytics here.",
                        ),
                )
                .into_any_element();
        }

        div()
            .id("analytics-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_analytics_listening_row(&summary, cx))
            .child(self.render_analytics_weekly_row(&summary, cx))
            .child(self.render_analytics_top_row(&summary, cx))
            .child(self.render_analytics_library_row(&summary, cx))
            .child(self.render_analytics_quality_row(&summary, cx))
            .child(self.render_analytics_recent_row(&summary, cx))
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: heatmap + hour-of-day clock
    // -------------------------------------------------------------------

    fn render_analytics_listening_row(
        &self,
        summary: &AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        // Calendar heatmap. Cells are arranged column-major: each
        // column is a week, rows are weekdays (Mon..Sun).
        let weeks = ANALYTICS_HEATMAP_DAYS.div_ceil(7);
        let total_cells = weeks * 7;
        let mut cells = vec![HeatmapCell::empty(); total_cells];
        let max_day = summary
            .listening_per_day
            .iter()
            .map(|entry| entry.seconds)
            .fold(0_u64, u64::max);
        for (ix, entry) in summary.listening_per_day.iter().enumerate() {
            let column_from_right = ix / 7;
            let column = weeks.saturating_sub(1).saturating_sub(column_from_right);
            let row = entry.weekday_row;
            let cell_ix = column * 7 + row;
            if cell_ix < cells.len() {
                let intensity = if max_day > 0 {
                    (entry.seconds as f32) / (max_day as f32)
                } else {
                    0.0
                };
                let tooltip = if entry.seconds > 0 {
                    Some(SharedString::from(format!(
                        "{} · {}",
                        entry.date.format("%a %b %d, %Y"),
                        format_hours_long(entry.seconds),
                    )))
                } else {
                    Some(SharedString::from(format!(
                        "{} · no listening",
                        entry.date.format("%a %b %d, %Y"),
                    )))
                };
                cells[cell_ix] = HeatmapCell {
                    intensity,
                    tooltip,
                    is_empty: entry.seconds == 0,
                };
            }
        }

        let mut heatmap_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-playback-hours"),
        };
        let heatmap_el = heatmap(
            &cells,
            7,
            weeks,
            10.0,
            3.0,
            colors.button,
            colors.accent,
            colors,
            Some(&mut heatmap_ctx),
            "playback",
        );

        let heatmap_panel = panel(colors)
            .flex_1()
            .min_w(px(420.0))
            .flex()
            .flex_col()
            .child(section_header(
                "PLAYBACK HOURS",
                Some(SharedString::from("last 365 days")),
                Some(SharedString::from(format!(
                    "{} total",
                    format_hours(summary.total_listened_secs)
                ))),
                colors,
            ))
            .child(heatmap_el)
            .child(
                div()
                    .px_4()
                    .pb_4()
                    .flex()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child("LESS")
                    .child(swatch(colors.button))
                    .child(swatch(blend_color(colors.button, colors.accent, 0.33)))
                    .child(swatch(blend_color(colors.button, colors.accent, 0.66)))
                    .child(swatch(colors.accent))
                    .child("MORE"),
            );

        let hour_panel = panel(colors)
            .min_w(px(280.0))
            .max_w(px(320.0))
            .flex_none()
            .flex()
            .flex_col()
            .child(section_header(
                "HOUR OF DAY",
                Some(SharedString::from("plays by start time")),
                None,
                colors,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb_4()
                    .child(radial_hours(&summary.hours_of_day, 220.0, colors)),
            );

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(heatmap_panel)
            .child(hour_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: weekday × hour grid + listening over time (sparkline)
    // -------------------------------------------------------------------

    fn render_analytics_weekly_row(
        &self,
        summary: &AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        // Listening clock: 7 rows (Mon..Sun) × 24 cols (00..23).
        // We have to manually reorder because the heatmap component
        // expects column-major data while we have row-major.
        const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let mut cells = Vec::with_capacity(7 * 24);
        let max = summary
            .weekday_hour_grid
            .iter()
            .copied()
            .fold(0_u64, u64::max);
        for col in 0..24 {
            for (row, weekday_name) in WEEKDAY_NAMES.iter().enumerate() {
                let val = summary.weekday_hour_grid[row * 24 + col];
                let intensity = if max > 0 {
                    (val as f32) / (max as f32)
                } else {
                    0.0
                };
                let tooltip = if val > 0 {
                    Some(SharedString::from(format!(
                        "{} · {:02}:00 · {}",
                        weekday_name,
                        col,
                        format_hours_long(val),
                    )))
                } else {
                    Some(SharedString::from(format!(
                        "{} · {:02}:00 · no listening",
                        weekday_name, col,
                    )))
                };
                cells.push(HeatmapCell {
                    intensity,
                    tooltip,
                    is_empty: val == 0,
                });
            }
        }

        let mut clock_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-listening-clock"),
        };
        let clock_el = heatmap(
            &cells,
            7,
            24,
            14.0,
            3.0,
            colors.button,
            colors.accent,
            colors,
            Some(&mut clock_ctx),
            "clock",
        );
        let cx = clock_ctx.cx;

        let weekday_panel = panel(colors)
            .flex_1()
            .min_w(px(420.0))
            .flex()
            .flex_col()
            .child(section_header(
                "LISTENING CLOCK",
                Some(SharedString::from("hour × weekday")),
                None,
                colors,
            ))
            .child(clock_el);

        // Per-week sparkline + label.
        let weekly_values: Vec<f64> = summary
            .listening_per_week
            .iter()
            .map(|secs| (*secs as f64) / 3600.0)
            .collect();

        // Build per-point tooltips. Each point covers a calendar week
        // ending on the corresponding entry's last day; we approximate
        // by counting weeks-ago-from-now since `compute_summary`
        // already aligns the buckets that way.
        let weekly_tooltips: Vec<Option<SharedString>> = summary
            .listening_per_week
            .iter()
            .enumerate()
            .map(|(ix, secs)| {
                let weeks_ago = summary.listening_per_week.len().saturating_sub(1) - ix;
                let label = if weeks_ago == 0 {
                    "this week".to_string()
                } else if weeks_ago == 1 {
                    "1 week ago".to_string()
                } else {
                    format!("{weeks_ago} weeks ago")
                };
                Some(SharedString::from(format!(
                    "{} · {}",
                    label,
                    format_hours_long(*secs),
                )))
            })
            .collect();

        let mut weekly_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-listening-over-time"),
        };
        let weekly_sparkline = sparkline(
            &weekly_values,
            320.0,
            140.0,
            colors,
            Some(&mut weekly_ctx),
            &weekly_tooltips,
            "weekly",
        );

        let weekly_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "LISTENING OVER TIME",
                Some(SharedString::from("hours per week")),
                Some(SharedString::from(if weekly_values.is_empty() {
                    "—".to_string()
                } else {
                    let last = *weekly_values.last().unwrap_or(&0.0);
                    format!("this wk {:.1}h", last)
                })),
                colors,
            ))
            .child(
                div()
                    .px_4()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(weekly_sparkline)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child(
                                summary
                                    .weekly_start_label
                                    .clone()
                                    .unwrap_or_else(|| SharedString::from("—")),
                            )
                            .child(
                                summary
                                    .weekly_end_label
                                    .clone()
                                    .unwrap_or_else(|| SharedString::from("now")),
                            ),
                    ),
            );

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(weekday_panel)
            .child(weekly_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: top artists / top albums / top tracks
    // -------------------------------------------------------------------

    fn render_analytics_top_row(
        &self,
        summary: &AnalyticsSummary,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        // Top Artists / Top Albums / Top Tracks intentionally omit
        // hover tooltips: every datum (artist/album/track name, play
        // count, hours, rank, strength bar) is already visible in the
        // ranked-list rows themselves, so a tooltip would just repeat
        // what's on screen. We still build the rows with `image: Some`
        // so each row gets the inline thumbnail.
        let artist_rows: Vec<RankedRow> = summary
            .top_artists
            .iter()
            .enumerate()
            .map(|(ix, entry)| RankedRow {
                rank: ix + 1,
                image: Some(self.analytics_row_image_for_artist(entry.name.as_ref())),
                primary: entry.name.clone(),
                secondary: None,
                value_primary: SharedString::from(format!("{} plays", entry.plays)),
                value_secondary: Some(SharedString::from(format_hours(entry.listened_secs))),
                ratio: entry.ratio,
                tooltip: None,
            })
            .collect();
        let artists_list = ranked_list(
            &artist_rows,
            &["#", "ARTIST", "PLAYS", "HRS", ""],
            colors,
            None,
            "artists",
        );
        let artists_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "TOP ARTISTS",
                Some(SharedString::from("by play count")),
                None,
                colors,
            ))
            .child(artists_list);

        let album_rows: Vec<RankedRow> = summary
            .top_albums
            .iter()
            .enumerate()
            .map(|(ix, entry)| RankedRow {
                rank: ix + 1,
                image: Some(self.analytics_row_image_for_album(
                    entry.title.as_ref(),
                    entry.artist.as_ref(),
                    entry.sample_track_path.as_deref(),
                )),
                primary: entry.title.clone(),
                secondary: Some(entry.artist.clone()),
                value_primary: SharedString::from(format!("{} plays", entry.plays)),
                value_secondary: Some(SharedString::from(format_hours(entry.listened_secs))),
                ratio: entry.ratio,
                tooltip: None,
            })
            .collect();
        let albums_list = ranked_list(
            &album_rows,
            &["#", "ALBUM", "PLAYS", "HRS", ""],
            colors,
            None,
            "albums",
        );
        let albums_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "TOP ALBUMS",
                Some(SharedString::from("by play count")),
                None,
                colors,
            ))
            .child(albums_list);

        let track_rows: Vec<RankedRow> = summary
            .top_tracks
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let image = self
                    .find_track_for_path(entry.track_path.as_path())
                    .map(row_image_for_track)
                    .unwrap_or_else(|| {
                        self.analytics_row_image_for_album(
                            entry.title.as_ref(),
                            entry.artist.as_ref(),
                            None,
                        )
                    });
                RankedRow {
                    rank: ix + 1,
                    image: Some(image),
                    primary: entry.title.clone(),
                    secondary: Some(entry.artist.clone()),
                    value_primary: SharedString::from(format!("{} plays", entry.plays)),
                    value_secondary: Some(SharedString::from(format_hours(entry.listened_secs))),
                    ratio: entry.ratio,
                    tooltip: None,
                }
            })
            .collect();
        let tracks_list = ranked_list(
            &track_rows,
            &["#", "TRACK", "PLAYS", "HRS", ""],
            colors,
            None,
            "tracks",
        );
        let tracks_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "TOP TRACKS",
                Some(SharedString::from("by play count")),
                None,
                colors,
            ))
            .child(tracks_list);

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(artists_panel)
            .child(albums_panel)
            .child(tracks_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: genre breakdown + file format split + decades
    // -------------------------------------------------------------------

    fn render_analytics_library_row(
        &self,
        summary: &AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        // Genre breakdown intentionally omits hover tooltips: the
        // value column already shows `count · hours` next to each
        // label, so a tooltip would just repeat the visible row.
        let genre_rows: Vec<HBarRow> = summary
            .top_genres
            .iter()
            .map(|entry| HBarRow {
                label: entry.label.clone(),
                value_label: SharedString::from(format!(
                    "{} · {}",
                    Self::format_count_short(entry.count),
                    format_hours(entry.total_secs)
                )),
                ratio: entry.ratio,
                color: None,
                tooltip: None,
            })
            .collect();

        let genre_list = h_bar_list(&genre_rows, colors.button, colors.accent, colors, None);

        let genre_panel = panel(colors)
            .flex_1()
            .min_w(px(360.0))
            .flex()
            .flex_col()
            .child(section_header(
                "GENRE BREAKDOWN",
                Some(SharedString::from("by listening hours")),
                Some(SharedString::from(format!(
                    "{} genres",
                    summary.unique_genres
                ))),
                colors,
            ))
            .child(genre_list);

        let by_count: Vec<StackSegment> = summary
            .file_formats
            .iter()
            .map(|entry| StackSegment {
                label: entry.label.clone(),
                value: entry.count as f64,
                color: entry.color,
                tooltip: Some(SharedString::from(format!(
                    "{} · {} files · {:.1}%",
                    entry.label,
                    Self::format_count_short(entry.count),
                    entry.share * 100.0,
                ))),
            })
            .collect();
        let by_size: Vec<StackSegment> = summary
            .file_formats
            .iter()
            .map(|entry| StackSegment {
                label: entry.label.clone(),
                value: entry.total_bytes as f64,
                color: entry.color,
                tooltip: Some(SharedString::from(format!(
                    "{} · {}",
                    entry.label,
                    Self::format_library_size_bytes(entry.total_bytes),
                ))),
            })
            .collect();

        let format_legend: Vec<LegendItem> = summary
            .file_formats
            .iter()
            .map(|entry| LegendItem {
                label: entry.label.clone(),
                value_label: SharedString::from(format!(
                    "{} · {:.1}%",
                    Self::format_count_short(entry.count),
                    entry.share * 100.0
                )),
                color: entry.color,
            })
            .collect();

        let mut count_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-format-count"),
        };
        let count_bar = stacked_bar(&by_count, 0.04, 18.0, colors, Some(&mut count_ctx), "count");
        let cx = count_ctx.cx;
        let mut size_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-format-size"),
        };
        let size_bar = stacked_bar(&by_size, 0.04, 18.0, colors, Some(&mut size_ctx), "size");
        let cx = size_ctx.cx;

        let format_panel = panel(colors)
            .flex_1()
            .min_w(px(360.0))
            .flex()
            .flex_col()
            .child(section_header(
                "FILE FORMAT SPLIT",
                Some(SharedString::from("count vs disk size")),
                Some(SharedString::from(format!(
                    "{} files",
                    Self::format_count_short(self.tracks.len())
                ))),
                colors,
            ))
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child("BY FILE COUNT"),
                    )
                    .child(count_bar)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child("BY DISK SIZE"),
                    )
                    .child(size_bar),
            )
            .child(legend_grid(&format_legend, colors));

        let decade_bars: Vec<VBar> = summary
            .decades
            .iter()
            .map(|entry| VBar {
                label: entry.label.clone(),
                value: entry.count as f64,
                value_label: Some(SharedString::from(Self::format_count_short(entry.count))),
                tooltip: Some(SharedString::from(format!(
                    "{} · {} tracks",
                    entry.label,
                    Self::format_count_short(entry.count),
                ))),
            })
            .collect();
        let mut decade_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-decades"),
        };
        let decade_chart = vertical_bar_chart(
            &decade_bars,
            160.0,
            colors.button_hover,
            true,
            colors,
            Some(&mut decade_ctx),
            "decade",
        );
        let decade_panel = panel(colors)
            .flex_1()
            .min_w(px(280.0))
            .flex()
            .flex_col()
            .child(section_header(
                "DECADES",
                Some(SharedString::from("release year")),
                None,
                colors,
            ))
            .child(decade_chart);

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(genre_panel)
            .child(format_panel)
            .child(decade_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: bitrate distribution + sample rate donut + size buckets donut
    // -------------------------------------------------------------------

    fn render_analytics_quality_row(
        &self,
        summary: &AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        let bitrate_bars: Vec<VBar> = summary
            .bitrate_buckets
            .iter()
            .map(|entry| VBar {
                label: entry.label.clone(),
                value: entry.count as f64,
                value_label: None,
                tooltip: Some(SharedString::from(format!(
                    "{} kbps · {} tracks",
                    entry.label,
                    Self::format_count_short(entry.count),
                ))),
            })
            .collect();
        let mut bitrate_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-bitrate"),
        };
        let bitrate_chart = vertical_bar_chart(
            &bitrate_bars,
            160.0,
            colors.accent_soft,
            true,
            colors,
            Some(&mut bitrate_ctx),
            "bitrate",
        );
        let _cx = bitrate_ctx.cx;
        let bitrate_panel = panel(colors)
            .flex_1()
            .min_w(px(300.0))
            .flex()
            .flex_col()
            .child(section_header(
                "BITRATE DISTRIBUTION",
                Some(SharedString::from("kbps")),
                None,
                colors,
            ))
            .child(bitrate_chart);

        // Sample rate donut.
        let sr_slices: Vec<DonutSlice> = summary
            .sample_rate_buckets
            .iter()
            .map(|entry| DonutSlice {
                value: entry.count as f64,
                color: entry.color,
                tooltip: None,
            })
            .collect();
        let sr_legend: Vec<LegendItem> = summary
            .sample_rate_buckets
            .iter()
            .map(|entry| LegendItem {
                label: entry.label.clone(),
                value_label: SharedString::from(Self::format_count_short(entry.count)),
                color: entry.color,
            })
            .collect();
        let sr_panel = panel(colors)
            .flex_1()
            .min_w(px(280.0))
            .flex()
            .flex_col()
            .child(section_header(
                "SAMPLE RATE",
                Some(SharedString::from("by track count")),
                None,
                colors,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_4()
                    .child(donut_svg(&sr_slices, 160.0, 22.0, colors)),
            )
            .child(legend_grid(&sr_legend, colors));

        // File size donut.
        let size_slices: Vec<DonutSlice> = summary
            .size_buckets
            .iter()
            .map(|entry| DonutSlice {
                value: entry.count as f64,
                color: entry.color,
                tooltip: None,
            })
            .collect();
        let size_legend: Vec<LegendItem> = summary
            .size_buckets
            .iter()
            .map(|entry| LegendItem {
                label: entry.label.clone(),
                value_label: SharedString::from(Self::format_count_short(entry.count)),
                color: entry.color,
            })
            .collect();

        let size_panel = panel(colors)
            .flex_1()
            .min_w(px(280.0))
            .flex()
            .flex_col()
            .child(section_header(
                "FILE SIZE",
                Some(SharedString::from("count by bucket")),
                None,
                colors,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_4()
                    .child(donut_svg(&size_slices, 160.0, 22.0, colors)),
            )
            .child(legend_grid(&size_legend, colors));

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(bitrate_panel)
            .child(sr_panel)
            .child(size_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Row: gathering dust + library growth
    // -------------------------------------------------------------------

    fn render_analytics_recent_row(
        &self,
        summary: &AnalyticsSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        // Same rationale as Top Artists / Albums / Tracks above:
        // the table already shows track + artist + plays + last
        // played, so a hover tooltip would just repeat those values.
        let dust_rows: Vec<RankedRow> = summary
            .gathering_dust
            .iter()
            .enumerate()
            .map(|(ix, entry)| RankedRow {
                rank: ix + 1,
                image: self
                    .find_track_for_path(entry.track_path.as_path())
                    .map(row_image_for_track),
                primary: entry.title.clone(),
                secondary: Some(entry.artist.clone()),
                value_primary: SharedString::from(format!("{} plays", entry.plays)),
                value_secondary: Some(entry.last_played.clone()),
                ratio: 0.0,
                tooltip: None,
            })
            .collect();

        let dust_list = ranked_list(
            &dust_rows,
            &["#", "TRACK", "PLAYS", "LAST", ""],
            colors,
            None,
            "dust",
        );

        let dust_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "GATHERING DUST",
                Some(SharedString::from("frequently played, not lately")),
                None,
                colors,
            ))
            .child(dust_list);

        let growth_values: Vec<f64> = summary.library_growth.iter().map(|v| *v as f64).collect();
        // Build a "MMM YYYY" label per growth bucket so the sparkline
        // tooltip can name the month and cumulative track count for
        // each data point.
        let growth_tooltips: Vec<Option<SharedString>> = {
            let now = Local::now();
            growth_values
                .iter()
                .enumerate()
                .map(|(ix, value)| {
                    let month_offset = (growth_values.len() - 1 - ix) as i32;
                    let mut year = now.year();
                    let mut month = now.month() as i32 - month_offset;
                    while month <= 0 {
                        month += 12;
                        year -= 1;
                    }
                    Some(SharedString::from(format!(
                        "{} {} · {} tracks",
                        month_short_name(month as u32),
                        year,
                        Self::format_count_short(*value as usize),
                    )))
                })
                .collect()
        };

        let mut growth_ctx = ChartCtx {
            app: self,
            cx,
            id_prefix: SharedString::from("analytics-library-growth"),
        };
        let growth_sparkline = sparkline(
            &growth_values,
            320.0,
            140.0,
            colors,
            Some(&mut growth_ctx),
            &growth_tooltips,
            "growth",
        );

        let growth_panel = panel(colors)
            .flex_1()
            .min_w(px(320.0))
            .flex()
            .flex_col()
            .child(section_header(
                "LIBRARY GROWTH",
                Some(SharedString::from("36 months")),
                Some(SharedString::from(format!(
                    "{} now",
                    Self::format_count_short(self.tracks.len())
                ))),
                colors,
            ))
            .child(
                div()
                    .px_4()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(growth_sparkline)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child(summary.growth_start_label.clone())
                            .child(summary.growth_end_label.clone()),
                    ),
            );

        div()
            .flex()
            .gap_3()
            .flex_wrap()
            .items_start()
            .child(dust_panel)
            .child(growth_panel)
            .into_any_element()
    }

    // -------------------------------------------------------------------
    // Right-side filter sidebar
    // -------------------------------------------------------------------

    /// Persistent right sidebar shown only on the Analytics page.
    /// Houses the time-range filter and a few summary stats so the
    /// page itself can be content-heavy without horizontal real estate
    /// pressure on the visualizations. Toggle is mirrored on
    /// `analytics_sidebar_collapsed` so the user's preference sticks
    /// across navigations.
    pub(super) fn render_analytics_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let collapsed = self.analytics_sidebar_collapsed;

        if collapsed {
            return div()
                .id("analytics-sidebar-collapsed")
                .w(px(28.0))
                .flex_none()
                .border_l_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.panel))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors.text_muted))
                        .child("‹"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.analytics_sidebar_collapsed = false;
                    this.save_app_state();
                    cx.notify();
                }))
                .into_any_element();
        }

        let summary = compute_summary(
            &self.tracks,
            &self.playback_history,
            self.analytics_time_range,
        );

        div()
            .w(px(RIGHT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .overflow_hidden()
            .child(self.render_analytics_sidebar_header(cx))
            .child(self.render_analytics_filters(cx))
            .child(self.render_analytics_sidebar_summary(&summary))
            .into_any_element()
    }

    fn render_analytics_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();
        div()
            .h(px(50.0))
            .flex_none()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child("Filters"),
            )
            .child(
                self.sidebar_button("›", "analytics-sidebar-collapse")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.analytics_sidebar_collapsed = true;
                        this.save_app_state();
                        cx.notify();
                    })),
            )
    }

    fn render_analytics_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();
        let current = self.analytics_time_range;

        let range_buttons = AnalyticsTimeRange::all().into_iter().map(|range| {
            let active = range == current;
            let bg = if active { colors.accent } else { colors.button };
            let fg = if active {
                colors.transport_primary_fg
            } else {
                colors.text
            };

            div()
                .id(SharedString::from(format!(
                    "analytics-range-{}",
                    range.label()
                )))
                .flex_1()
                .h(px(30.0))
                .rounded_sm()
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(bg))
                .text_color(rgb(fg))
                .text_xs()
                .font_weight(gpui::FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |this| {
                    if active {
                        this.opacity(0.92)
                    } else {
                        this.bg(rgb(colors.button_hover))
                    }
                })
                .child(range.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.analytics_time_range != range {
                        this.analytics_time_range = range;
                        this.save_app_state();
                        cx.notify();
                    }
                }))
        });

        div()
            .px_4()
            .py_4()
            .flex()
            .flex_col()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_faint))
                    .child("TIME RANGE"),
            )
            .child(div().flex().gap_1().children(range_buttons))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(SharedString::from(self.analytics_time_range.long_label())),
            )
            .child(
                div()
                    .h(px(1.0))
                    .w_full()
                    .bg(rgb(colors.border_subtle))
                    .my_1(),
            )
            .child(div().text_xs().text_color(rgb(colors.text_faint)).child(
                "Library stats (genre, format, decades, …) always reflect the entire \
                         library. Listening stats use the time range above.",
            ))
    }

    fn render_analytics_sidebar_summary(&self, summary: &AnalyticsSummary) -> AnyElement {
        let colors = *self.colors();

        let total_hours = summary.total_listened_secs as f64 / 3600.0;
        let avg = summary.avg_hours_per_active_day;
        let unique_tracks = summary.unique_tracks_played;
        let library_size = Self::format_library_size_bytes(self.library_size_bytes);
        let library_hours = (summary.total_library_secs as f64 / 3600.0).round();

        div()
            .id("analytics-sidebar-summary")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(sidebar_stat(
                "LISTENING TIME",
                format!("{:.1} h", total_hours),
                Some(format!("{} plays", summary.total_plays)),
                colors,
            ))
            .child(sidebar_stat(
                "AVG / DAY",
                if avg > 0.0 {
                    format!("{:.2} h", avg)
                } else {
                    "—".to_string()
                },
                Some(format!("{} active days", summary.active_days)),
                colors,
            ))
            .child(sidebar_stat(
                "STREAK",
                format!("{} d", summary.current_streak_days),
                Some(format!("best {} d", summary.longest_streak_days)),
                colors,
            ))
            .child(sidebar_stat(
                "UNIQUE TRACKS",
                format!("{}", unique_tracks),
                Some(format!(
                    "{:.1}% of library",
                    if self.tracks.is_empty() {
                        0.0
                    } else {
                        unique_tracks as f64 / self.tracks.len() as f64 * 100.0
                    }
                )),
                colors,
            ))
            .child(div().h(px(1.0)).w_full().bg(rgb(colors.border_subtle)))
            .child(sidebar_stat(
                "LIBRARY",
                format!("{} files", Self::format_count_short(self.tracks.len())),
                Some(format!("{} hours · {}", library_hours, library_size)),
                colors,
            ))
            .child(sidebar_stat(
                "ARTISTS",
                Self::format_count_short(self.artists.len()),
                None,
                colors,
            ))
            .child(sidebar_stat(
                "ALBUMS",
                Self::format_count_short(self.albums.len()),
                None,
                colors,
            ))
            .child(sidebar_stat(
                "GENRES",
                Self::format_count_short(summary.unique_genres),
                Some(format!(
                    "missing {}",
                    Self::format_count_short(summary.missing_genre)
                )),
                colors,
            ))
            .child(sidebar_stat(
                "MISSING METADATA",
                Self::format_count_short(summary.missing_metadata_total()),
                Some(format!(
                    "{} no year · {} no artwork",
                    summary.missing_year, summary.missing_artwork,
                )),
                colors,
            ))
            .into_any_element()
    }
}

// ============================================================================
// Reusable, page-local sidebar primitive (small numeric stat).
// ============================================================================

/// Compact sidebar key/value tile. Lives here (not in `charts.rs`)
/// because it's a sidebar-shaped variant of `kpi_card` rather than a
/// chart primitive.
fn sidebar_stat(
    label: &'static str,
    value: impl Into<SharedString>,
    footnote: Option<String>,
    colors: super::theme::ThemeColors,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(colors.text_faint))
                .child(label),
        )
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(colors.text_strong))
                .child(value.into()),
        )
        .when_some(footnote, |this, footnote| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(footnote),
            )
        })
        .into_any_element()
}

// ============================================================================
// Aggregation
// ============================================================================

/// All pre-aggregated values backing the analytics page. Computed
/// once per render from `tracks` + `playback_history`. Pure data, so
/// adding tests for `compute_summary()` later is straightforward.
struct AnalyticsSummary {
    // -- Library --------------------------------------------------------
    total_library_secs: u64,
    unique_genres: usize,
    top_genres: Vec<GenreEntry>,
    file_formats: Vec<FormatEntry>,
    bitrate_buckets: Vec<BucketEntry>,
    sample_rate_buckets: Vec<SizeBucketEntry>,
    decades: Vec<DecadeEntry>,
    size_buckets: Vec<SizeBucketEntry>,
    library_growth: Vec<u32>,
    growth_start_label: SharedString,
    growth_end_label: SharedString,
    missing_genre: usize,
    missing_year: usize,
    missing_artwork: usize,

    // -- Listening (time-filtered) -------------------------------------
    total_listened_secs: u64,
    total_plays: usize,
    active_days: usize,
    avg_hours_per_active_day: f64,
    current_streak_days: u32,
    longest_streak_days: u32,
    unique_tracks_played: usize,

    listening_per_day: Vec<DayEntry>,
    listening_per_week: Vec<u64>,
    weekly_start_label: Option<SharedString>,
    weekly_end_label: Option<SharedString>,
    hours_of_day: [f64; 24],
    weekday_hour_grid: [u64; 7 * 24],
    top_artists: Vec<ArtistEntry>,
    top_albums: Vec<AlbumEntry>,
    top_tracks: Vec<TrackEntry>,
    gathering_dust: Vec<DustEntry>,
}

impl AnalyticsSummary {
    /// Total tracks missing at least one of: genre, year, or artwork.
    /// Counted independently per dimension, so a track missing two
    /// fields contributes twice. That matches the "things to fix"
    /// counter the sidebar wants to show, rather than a strict
    /// distinct-row count.
    fn missing_metadata_total(&self) -> usize {
        self.missing_genre + self.missing_year + self.missing_artwork
    }
}

// ----- entry structs --------------------------------------------------------

struct GenreEntry {
    label: SharedString,
    count: usize,
    total_secs: u64,
    ratio: f32,
}

struct FormatEntry {
    label: SharedString,
    count: usize,
    total_bytes: u64,
    share: f32,
    color: u32,
}

struct DayEntry {
    /// 0 = Mon, 6 = Sun.
    weekday_row: usize,
    seconds: u64,
    /// Calendar date this entry represents. Carried alongside the
    /// playback time so the heatmap renderer can build a per-cell
    /// tooltip without re-deriving dates from indices.
    date: NaiveDate,
}

struct BucketEntry {
    label: SharedString,
    count: usize,
}

struct DecadeEntry {
    label: SharedString,
    count: usize,
}

struct ArtistEntry {
    name: SharedString,
    plays: u32,
    listened_secs: u64,
    ratio: f32,
}

struct AlbumEntry {
    title: SharedString,
    artist: SharedString,
    plays: u32,
    listened_secs: u64,
    ratio: f32,
    /// Track path of any track from this album, used to resolve cover
    /// artwork at render time.
    sample_track_path: Option<std::path::PathBuf>,
}

struct TrackEntry {
    title: SharedString,
    artist: SharedString,
    plays: u32,
    listened_secs: u64,
    ratio: f32,
    /// Path to the track itself; used to look up the underlying
    /// `Track` for inline artwork.
    track_path: std::path::PathBuf,
}

struct SizeBucketEntry {
    label: SharedString,
    count: usize,
    color: u32,
}

struct DustEntry {
    track_path: std::path::PathBuf,
    title: SharedString,
    artist: SharedString,
    plays: u32,
    last_played: SharedString,
}

// ============================================================================

fn compute_summary(
    tracks: &[Track],
    history: &[super::PlaybackHistoryEntry],
    range: AnalyticsTimeRange,
) -> AnalyticsSummary {
    let now = Local::now();

    // ------------------------------------------------------------------
    // Library-side aggregations (always whole library).
    // ------------------------------------------------------------------
    let mut genre_counts: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut format_counts: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut decade_counts: BTreeMap<i32, usize> = BTreeMap::new();
    let mut bitrate_buckets: Vec<(usize, &'static str)> = vec![
        (0, "<128"),
        (0, "128–191"),
        (0, "192–255"),
        (0, "256–319"),
        (0, "320"),
        (0, "lossless"),
    ];
    let mut size_bucket_counts = [0_usize; 5];
    let mut sample_rate_counts: BTreeMap<u32, usize> = BTreeMap::new();
    let mut total_library_secs: u64 = 0;
    let mut total_size_bytes: u64 = 0;
    // Library growth uses a two-pass approach: collect raw `date_added`
    // seconds for each track first, then detect whether a single
    // timestamp dominates (the typical signature of a one-shot DB-init
    // / migration backfill). When that's the case, the dominant
    // timestamp is treated as a sentinel and those tracks fall back
    // to filesystem creation/modified time so the resulting curve
    // reflects when files actually entered the library rather than
    // when the catalog was built.
    let mut growth_inputs: Vec<(u64, &std::path::Path)> = Vec::with_capacity(tracks.len());
    let mut track_lookup: HashMap<&std::path::Path, &Track> = HashMap::with_capacity(tracks.len());
    let mut sample_track_for_album: HashMap<(String, String), std::path::PathBuf> = HashMap::new();
    let mut missing_genre = 0_usize;
    let mut missing_year = 0_usize;
    let mut missing_artwork = 0_usize;

    for track in tracks {
        track_lookup.insert(track.path.as_path(), track);

        // Track at most one path per (album, artist) so the analytics
        // page can resolve a representative cover for each album in
        // the "TOP ALBUMS" / "TOP TRACKS" panels without re-walking
        // `tracks` on every render.
        let album_key = (track.album.to_string(), track.artist.to_string());
        sample_track_for_album
            .entry(album_key)
            .or_insert_with(|| track.path.clone());

        let genre_str = track.genre.as_ref().to_string();
        if genre_is_missing(&genre_str) {
            missing_genre += 1;
        }
        let entry = genre_counts.entry(genre_str).or_default();
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(track.duration_value.as_secs());

        let format_key = file_extension_label(&track.path, &track.codec);
        let format_entry = format_counts.entry(format_key).or_default();
        format_entry.0 += 1;
        format_entry.1 = format_entry.1.saturating_add(track.file_size);

        let year = track.year.as_ref();
        if let Some(year) = parse_year(year) {
            let decade = year - (year.rem_euclid(10));
            *decade_counts.entry(decade).or_default() += 1;
        } else {
            missing_year += 1;
        }

        if track.artwork.is_none() {
            missing_artwork += 1;
        }

        match track.bitrate {
            Some(b) if b < 128 => bitrate_buckets[0].0 += 1,
            Some(b) if b < 192 => bitrate_buckets[1].0 += 1,
            Some(b) if b < 256 => bitrate_buckets[2].0 += 1,
            Some(b) if b < 320 => bitrate_buckets[3].0 += 1,
            Some(b) if b < 1000 => bitrate_buckets[4].0 += 1,
            Some(_) => bitrate_buckets[5].0 += 1,
            None => bitrate_buckets[5].0 += 1,
        }

        let mb = track.file_size / 1_000_000;
        let bucket_ix = match mb {
            0..=4 => 0,
            5..=14 => 1,
            15..=49 => 2,
            50..=199 => 3,
            _ => 4,
        };
        size_bucket_counts[bucket_ix] += 1;

        // Sample rate: bucket by common values; "other" catches the rest.
        // Tempo's `Track` doesn't track sample-rate today, so we infer
        // from codec heuristics: lossless-ish formats default to 44.1 kHz
        // and lossy to 44.1 kHz too. This keeps the panel useful even
        // before per-track sample-rate is wired through.
        let sr_key = sample_rate_key_for(&track.codec, track.bitrate);
        *sample_rate_counts.entry(sr_key).or_default() += 1;

        total_library_secs = total_library_secs.saturating_add(track.duration_value.as_secs());
        total_size_bytes = total_size_bytes.saturating_add(track.file_size);

        if let Ok(elapsed) = track.date_added.duration_since(std::time::UNIX_EPOCH) {
            growth_inputs.push((elapsed.as_secs(), track.path.as_path()));
        }
    }

    let growth_buckets = build_growth_buckets(&growth_inputs);

    // `total_size_bytes` is intentionally unused beyond the running
    // sum above — the sidebar reads `library_size_bytes` directly off
    // `TempoApp` rather than re-deriving it here.
    let _ = total_size_bytes;

    // ------------------------------------------------------------------
    // Top genres + share
    // ------------------------------------------------------------------
    let unique_genres = genre_counts.len();
    let mut sorted_genres: Vec<(String, usize, u64)> = genre_counts
        .into_iter()
        .map(|(label, (count, secs))| (label, count, secs))
        .collect();
    sorted_genres.sort_by_key(|g| std::cmp::Reverse(g.2));
    let max_genre_secs = sorted_genres
        .iter()
        .map(|(_, _, secs)| *secs)
        .max()
        .unwrap_or(0)
        .max(1);
    let top_genres: Vec<GenreEntry> = sorted_genres
        .into_iter()
        .take(8)
        .map(|(label, count, secs)| GenreEntry {
            label: SharedString::from(label),
            count,
            total_secs: secs,
            ratio: secs as f32 / max_genre_secs as f32,
        })
        .collect();

    // ------------------------------------------------------------------
    // Top file formats (with stable color palette).
    // ------------------------------------------------------------------
    let format_palette: [u32; 8] = [
        0xeeb17d, 0x6f9dff, 0x9bbdff, 0xa8d39e, 0xd9a3df, 0xf2c693, 0x7adfd1, 0xc7c7c7,
    ];
    let mut format_entries: Vec<(String, usize, u64)> = format_counts
        .into_iter()
        .map(|(label, (count, bytes))| (label, count, bytes))
        .collect();
    format_entries.sort_by_key(|f| std::cmp::Reverse(f.1));
    let total_format_count: usize = format_entries.iter().map(|(_, count, _)| *count).sum();
    let file_formats: Vec<FormatEntry> = format_entries
        .into_iter()
        .enumerate()
        .map(|(ix, (label, count, total_bytes))| FormatEntry {
            label: SharedString::from(label),
            count,
            total_bytes,
            share: if total_format_count > 0 {
                count as f32 / total_format_count as f32
            } else {
                0.0
            },
            color: format_palette[ix % format_palette.len()],
        })
        .collect();

    // ------------------------------------------------------------------
    // Decades histogram.
    // ------------------------------------------------------------------
    let decades: Vec<DecadeEntry> = decade_counts
        .into_iter()
        .map(|(year, count)| DecadeEntry {
            label: SharedString::from(format!("{}s", year)),
            count,
        })
        .collect();

    // Bitrate buckets.
    let bitrate_buckets: Vec<BucketEntry> = bitrate_buckets
        .into_iter()
        .map(|(count, label)| BucketEntry {
            label: SharedString::from(label),
            count,
        })
        .collect();

    // Sample rate buckets -> color-coded slices.
    let sr_palette: [u32; 6] = [0xa8d39e, 0x6f9dff, 0xeeb17d, 0xd9a3df, 0x7adfd1, 0xc7c7c7];
    let mut sr_entries: Vec<(u32, usize)> = sample_rate_counts.into_iter().collect();
    sr_entries.sort_by_key(|s| std::cmp::Reverse(s.1));
    let sample_rate_buckets: Vec<SizeBucketEntry> = sr_entries
        .into_iter()
        .enumerate()
        .map(|(ix, (rate, count))| SizeBucketEntry {
            label: SharedString::from(format_sample_rate(rate)),
            count,
            color: sr_palette[ix % sr_palette.len()],
        })
        .collect();

    // Library growth (last 36 months, cumulative).
    let mut growth: Vec<u32> = Vec::with_capacity(36);
    let mut cumulative: u32 = 0;
    let earliest_year = now.year() - 3;
    let earliest_month = now.month();
    let baseline: u32 = growth_buckets
        .iter()
        .filter(|((y, m), _)| *y < earliest_year || (*y == earliest_year && *m < earliest_month))
        .map(|(_, count)| *count)
        .sum();
    cumulative = cumulative.saturating_add(baseline);
    for offset in 0..36 {
        let month_offset = 35 - offset;
        let mut target_year = now.year() - (month_offset / 12);
        let mut target_month = now.month() as i32 - (month_offset % 12);
        if target_month <= 0 {
            target_month += 12;
            target_year -= 1;
        }
        if let Some(count) = growth_buckets.get(&(target_year, target_month as u32)) {
            cumulative = cumulative.saturating_add(*count);
        }
        growth.push(cumulative);
    }
    let growth_start_label = SharedString::from(format!("{}", now.year() - 3));
    let growth_end_label = SharedString::from(format!("{}", now.year()));

    // ------------------------------------------------------------------
    // Listening aggregations from playback history (time-filtered).
    // ------------------------------------------------------------------
    let cutoff_secs = range.window_days().map(|days| {
        now.timestamp()
            .saturating_sub((days as i64).saturating_mul(86_400)) as u64
    });

    let mut total_listened_secs: u64 = 0;
    let mut hours_of_day = [0.0_f64; 24];
    let mut weekday_hour_grid = [0_u64; 7 * 24];
    let mut listening_by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut artist_stats: BTreeMap<String, (u32, u64)> = BTreeMap::new();
    let mut album_stats: BTreeMap<(String, String), (u32, u64)> = BTreeMap::new();
    let mut track_stats: BTreeMap<std::path::PathBuf, (u32, u64, String, String)> = BTreeMap::new();
    let mut last_played_per_track: HashMap<std::path::PathBuf, u64> = HashMap::new();
    let mut all_time_track_stats: HashMap<std::path::PathBuf, (u32, u64, String, String)> =
        HashMap::new();
    let mut unique_tracks: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    for entry in history {
        let dur_secs = parse_duration_label(&entry.duration);

        // Always-aggregated stats (used by gathering-dust irrespective
        // of the time filter):
        last_played_per_track
            .entry(entry.track_path.clone())
            .and_modify(|t| *t = (*t).max(entry.played_at_unix_secs))
            .or_insert(entry.played_at_unix_secs);
        let track = track_lookup.get(entry.track_path.as_path()).copied();
        let artist_name = track
            .map(|t| t.artist.to_string())
            .unwrap_or_else(|| entry.artist.clone());
        let title = track
            .map(|t| t.title.to_string())
            .unwrap_or_else(|| entry.title.clone());
        let all_stat = all_time_track_stats
            .entry(entry.track_path.clone())
            .or_insert_with(|| (0, 0, title.clone(), artist_name.clone()));
        all_stat.0 += 1;
        all_stat.1 = all_stat.1.saturating_add(dur_secs);

        // Time-filtered stats.
        if let Some(cutoff) = cutoff_secs
            && entry.played_at_unix_secs < cutoff
        {
            continue;
        }

        unique_tracks.insert(entry.track_path.clone());
        total_listened_secs = total_listened_secs.saturating_add(dur_secs);

        let dt: DateTime<Local> = Local
            .timestamp_opt(entry.played_at_unix_secs as i64, 0)
            .single()
            .unwrap_or_else(Local::now);

        hours_of_day[dt.hour() as usize] += dur_secs as f64 / 3600.0;
        let weekday = dt.weekday().num_days_from_monday() as usize;
        weekday_hour_grid[weekday * 24 + dt.hour() as usize] =
            weekday_hour_grid[weekday * 24 + dt.hour() as usize].saturating_add(dur_secs);

        let date = dt.date_naive();
        *listening_by_day.entry(date).or_default() += dur_secs;

        let astat = artist_stats.entry(artist_name.clone()).or_default();
        astat.0 += 1;
        astat.1 = astat.1.saturating_add(dur_secs);

        let key = (entry.album.clone(), artist_name.clone());
        let bstat = album_stats.entry(key).or_default();
        bstat.0 += 1;
        bstat.1 = bstat.1.saturating_add(dur_secs);

        let tkey = entry.track_path.clone();
        let tstat = track_stats
            .entry(tkey)
            .or_insert_with(|| (0, 0, title.clone(), artist_name.clone()));
        tstat.0 += 1;
        tstat.1 = tstat.1.saturating_add(dur_secs);
    }

    // Per-day / per-week.
    let listening_per_day = build_per_day_entries(now, &listening_by_day);
    let active_days = listening_by_day.values().filter(|secs| **secs > 0).count();
    let avg_hours_per_active_day = if active_days > 0 {
        total_listened_secs as f64 / 3600.0 / active_days as f64
    } else {
        0.0
    };

    let weeks_to_show: usize = match range.window_days() {
        Some(days) => (days.div_ceil(7)).max(1) as usize,
        None => 26, // ~6 months
    };
    let listening_per_week = build_per_week_secs(now, &listening_by_day, weeks_to_show);
    let weekly_start_label = match range.window_days() {
        Some(days) => Some(SharedString::from(format!(
            "{}",
            (now - ChronoDuration::days(days as i64)).format("%b %d")
        ))),
        None => Some(SharedString::from(format!(
            "{}",
            (now - ChronoDuration::weeks(weeks_to_show as i64)).format("%b %Y")
        ))),
    };
    let weekly_end_label = Some(SharedString::from(format!("{}", now.format("%b %d"))));

    // Top artists.
    let mut artist_vec: Vec<(String, u32, u64)> = artist_stats
        .into_iter()
        .map(|(k, (plays, secs))| (k, plays, secs))
        .collect();
    artist_vec.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    let max_artist_plays = artist_vec
        .iter()
        .map(|(_, plays, _)| *plays)
        .max()
        .unwrap_or(0)
        .max(1);
    let top_artists: Vec<ArtistEntry> = artist_vec
        .into_iter()
        .take(10)
        .map(|(name, plays, secs)| ArtistEntry {
            name: SharedString::from(name),
            plays,
            listened_secs: secs,
            ratio: plays as f32 / max_artist_plays as f32,
        })
        .collect();

    // Top albums.
    let mut album_vec: Vec<((String, String), u32, u64)> = album_stats
        .into_iter()
        .map(|(k, (plays, secs))| (k, plays, secs))
        .collect();
    album_vec.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    let max_album_plays = album_vec
        .iter()
        .map(|(_, plays, _)| *plays)
        .max()
        .unwrap_or(0)
        .max(1);
    let top_albums: Vec<AlbumEntry> = album_vec
        .into_iter()
        .take(10)
        .map(|((title, artist), plays, secs)| {
            let sample_track_path = sample_track_for_album
                .get(&(title.clone(), artist.clone()))
                .cloned();
            AlbumEntry {
                title: SharedString::from(title),
                artist: SharedString::from(artist),
                plays,
                listened_secs: secs,
                ratio: plays as f32 / max_album_plays as f32,
                sample_track_path,
            }
        })
        .collect();

    // Top tracks.
    let mut track_vec: Vec<(std::path::PathBuf, u32, u64, String, String)> = track_stats
        .into_iter()
        .map(|(p, (plays, secs, title, artist))| (p, plays, secs, title, artist))
        .collect();
    track_vec.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    let max_track_plays = track_vec
        .iter()
        .map(|(_, plays, _, _, _)| *plays)
        .max()
        .unwrap_or(0)
        .max(1);
    let top_tracks: Vec<TrackEntry> = track_vec
        .into_iter()
        .take(10)
        .map(|(path, plays, secs, title, artist)| TrackEntry {
            title: SharedString::from(title),
            artist: SharedString::from(artist),
            plays,
            listened_secs: secs,
            ratio: plays as f32 / max_track_plays as f32,
            track_path: path,
        })
        .collect();

    // Gathering dust: high-play tracks not played in >= 30 days
    // (uses all-time stats so the list is meaningful even when the
    // time filter is short).
    let dust_cutoff = now.timestamp() as u64 - 30 * 86_400;
    let mut dust_candidates: Vec<(&std::path::Path, u32, u64, &str, &str, u64)> =
        all_time_track_stats
            .iter()
            .filter_map(|(path, (plays, _secs, title, artist))| {
                let last = *last_played_per_track.get(path)?;
                if *plays >= 5 && last <= dust_cutoff {
                    Some((
                        path.as_path(),
                        *plays,
                        last,
                        title.as_str(),
                        artist.as_str(),
                        last,
                    ))
                } else {
                    None
                }
            })
            .collect();
    // Order by play count (high first), then by stalest (oldest last).
    dust_candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    let gathering_dust: Vec<DustEntry> = dust_candidates
        .into_iter()
        .take(8)
        .map(
            |(path, plays, _last_secs, title, artist, last_played)| DustEntry {
                track_path: path.to_path_buf(),
                title: SharedString::from(title.to_string()),
                artist: SharedString::from(artist.to_string()),
                plays,
                last_played: SharedString::from(format_relative_time(last_played, now)),
            },
        )
        .collect();

    // Streaks (day-by-day from time-filtered listening_by_day).
    let (current_streak_days, longest_streak_days) =
        compute_streaks(now.date_naive(), &listening_by_day);

    // Size buckets.
    let size_palette: [u32; 5] = [0xa8d39e, 0xeeb17d, 0xf2c693, 0xd9a3df, 0xe25563];
    let size_labels = ["<5 MB", "5–15", "15–50", "50–200", "200+"];
    let size_buckets: Vec<SizeBucketEntry> = size_bucket_counts
        .iter()
        .enumerate()
        .map(|(ix, count)| SizeBucketEntry {
            label: SharedString::from(size_labels[ix]),
            count: *count,
            color: size_palette[ix],
        })
        .collect();

    AnalyticsSummary {
        total_library_secs,
        unique_genres,
        top_genres,
        file_formats,
        bitrate_buckets,
        sample_rate_buckets,
        decades,
        size_buckets,
        library_growth: growth,
        growth_start_label,
        growth_end_label,
        missing_genre,
        missing_year,
        missing_artwork,

        total_listened_secs,
        total_plays: history
            .iter()
            .filter(|entry| match cutoff_secs {
                Some(cutoff) => entry.played_at_unix_secs >= cutoff,
                None => true,
            })
            .count(),
        active_days,
        avg_hours_per_active_day,
        current_streak_days,
        longest_streak_days,
        unique_tracks_played: unique_tracks.len(),

        listening_per_day,
        listening_per_week,
        weekly_start_label,
        weekly_end_label,
        hours_of_day,
        weekday_hour_grid,
        top_artists,
        top_albums,
        top_tracks,
        gathering_dust,
    }
}

// --- helpers ---------------------------------------------------------------

/// Minimum share of the library that has to share a single
/// `date_added` second-resolution timestamp before that timestamp is
/// treated as a DB-init / migration sentinel. 30% picks up the
/// typical "all rows have the same timestamp" pattern from the
/// `add_column_if_missing` migration without misclassifying a normal
/// day where many files were added at once.
const SENTINEL_SHARE_THRESHOLD: f64 = 0.30;
const SENTINEL_MIN_TRACKS: usize = 50;

/// Bucket library-growth inputs by `(year, month)`, falling back to
/// filesystem creation/modified time for tracks whose `date_added`
/// matches a detected DB-init sentinel timestamp.
fn build_growth_buckets(inputs: &[(u64, &std::path::Path)]) -> BTreeMap<(i32, u32), u32> {
    use std::collections::HashMap as Map;

    let total = inputs.len();
    let mut counts: Map<u64, usize> = Map::with_capacity(total);
    for (secs, _) in inputs {
        *counts.entry(*secs).or_default() += 1;
    }
    let sentinel = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| {
            *count >= SENTINEL_MIN_TRACKS
                && (*count as f64) / (total.max(1) as f64) >= SENTINEL_SHARE_THRESHOLD
        })
        .map(|(secs, _)| secs);

    let mut buckets: BTreeMap<(i32, u32), u32> = BTreeMap::new();
    for (secs, path) in inputs {
        let bucket_secs = if Some(*secs) == sentinel {
            fs_birth_or_mtime_secs(path).unwrap_or(*secs)
        } else {
            *secs
        };
        if let Some(dt) = Local.timestamp_opt(bucket_secs as i64, 0).single() {
            *buckets.entry((dt.year(), dt.month())).or_default() += 1;
        }
    }
    buckets
}

/// Best-effort filesystem timestamp for a track. Prefers `created`
/// (the file's birth time, when the platform exposes one) and falls
/// back to `modified`. Returns seconds since the Unix epoch.
fn fs_birth_or_mtime_secs(path: &std::path::Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let stamp = metadata
        .created()
        .ok()
        .or_else(|| metadata.modified().ok())?;
    stamp
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn build_per_day_entries(
    now: DateTime<Local>,
    listening_by_day: &BTreeMap<NaiveDate, u64>,
) -> Vec<DayEntry> {
    let today = now.date_naive();
    (0..ANALYTICS_HEATMAP_DAYS)
        .map(|days_ago| {
            let date = today
                .checked_sub_signed(chrono::Duration::days(days_ago as i64))
                .unwrap_or(today);
            let seconds = listening_by_day.get(&date).copied().unwrap_or(0);
            let weekday_row = date.weekday().num_days_from_monday() as usize;
            DayEntry {
                weekday_row,
                seconds,
                date,
            }
        })
        .collect()
}

fn build_per_week_secs(
    now: DateTime<Local>,
    listening_by_day: &BTreeMap<NaiveDate, u64>,
    weeks: usize,
) -> Vec<u64> {
    let today = now.date_naive();
    (0..weeks)
        .rev()
        .map(|week_ix| {
            let mut sum: u64 = 0;
            for day_offset in 0..7 {
                let days_ago = (week_ix * 7) + day_offset;
                if let Some(date) =
                    today.checked_sub_signed(chrono::Duration::days(days_ago as i64))
                {
                    sum = sum.saturating_add(listening_by_day.get(&date).copied().unwrap_or(0));
                }
            }
            sum
        })
        .collect()
}

fn compute_streaks(today: NaiveDate, by_day: &BTreeMap<NaiveDate, u64>) -> (u32, u32) {
    let mut current = 0_u32;
    let mut cursor = today;
    loop {
        if matches!(by_day.get(&cursor), Some(secs) if *secs > 0) {
            current += 1;
            match cursor.checked_sub_signed(chrono::Duration::days(1)) {
                Some(prev) => cursor = prev,
                None => break,
            }
        } else if current == 0 && cursor == today {
            // Today might not have a play yet; allow yesterday to seed streak.
            match cursor.checked_sub_signed(chrono::Duration::days(1)) {
                Some(prev) => cursor = prev,
                None => break,
            }
        } else {
            break;
        }
    }
    let mut longest = 0_u32;
    let mut run = 0_u32;
    let mut prev_date: Option<NaiveDate> = None;
    for (date, secs) in by_day.iter() {
        if *secs == 0 {
            continue;
        }
        match prev_date {
            Some(prev) if *date == prev + chrono::Duration::days(1) => {
                run += 1;
            }
            _ => {
                run = 1;
            }
        }
        longest = longest.max(run);
        prev_date = Some(*date);
    }
    (current, longest)
}

fn file_extension_label(path: &std::path::Path, codec: &str) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_uppercase());
    ext.filter(|label| !label.is_empty())
        .unwrap_or_else(|| codec.to_uppercase())
}

fn parse_year(year: &str) -> Option<i32> {
    let trimmed = year.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let parsed = trimmed.parse::<i32>().ok()?;
    if (1900..=2100).contains(&parsed) {
        Some(parsed)
    } else {
        None
    }
}

/// Parse a `m:ss` or `h:mm:ss` formatted duration label produced by
/// the player's `format_duration` helper into seconds. Anything we
/// can't parse becomes 0.
fn parse_duration_label(label: &str) -> u64 {
    let parts: Vec<&str> = label.split(':').collect();
    let parsed: Option<Vec<u64>> = parts
        .iter()
        .map(|piece| piece.trim().parse::<u64>().ok())
        .collect();
    let parsed = match parsed {
        Some(parts) => parts,
        None => return 0,
    };
    match parsed.len() {
        2 => parsed[0] * 60 + parsed[1],
        3 => parsed[0] * 3600 + parsed[1] * 60 + parsed[2],
        _ => 0,
    }
}

fn format_hours(seconds: u64) -> String {
    let hours = seconds as f64 / 3600.0;
    if hours >= 10.0 {
        format!("{:.0}h", hours)
    } else if hours >= 1.0 {
        format!("{:.1}h", hours)
    } else {
        format!("{}m", seconds / 60)
    }
}

/// More verbose duration formatter used in tooltips: prefers
/// `"H h M m"` when there are at least a few minutes of listening,
/// falls back to plain minutes otherwise.
fn format_hours_long(seconds: u64) -> String {
    let total_minutes = seconds / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours == 0 {
        format!("{minutes} min")
    } else if minutes == 0 {
        format!("{hours} h")
    } else {
        format!("{hours} h {minutes} m")
    }
}

fn month_short_name(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "?",
    }
}

fn format_relative_time(played_at_secs: u64, now: DateTime<Local>) -> String {
    let now_secs = now.timestamp() as u64;
    if played_at_secs > now_secs {
        return "now".to_string();
    }
    let diff = now_secs - played_at_secs;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 30 * 86_400 {
        format!("{}d ago", diff / 86_400)
    } else {
        let dt = Local
            .timestamp_opt(played_at_secs as i64, 0)
            .single()
            .unwrap_or(now);
        dt.format("%b %d").to_string()
    }
}

fn genre_is_missing(genre: &str) -> bool {
    let trimmed = genre.trim();
    trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("unknown")
        || trimmed.eq_ignore_ascii_case("unknown genre")
        || trimmed.eq_ignore_ascii_case("none")
}

fn sample_rate_key_for(codec: &str, bitrate: Option<u32>) -> u32 {
    let lossless = matches!(
        codec.to_ascii_uppercase().as_str(),
        "FLAC" | "ALAC" | "WAV" | "AIFF"
    );
    match (lossless, bitrate.unwrap_or(0)) {
        (true, b) if b > 1500 => 96_000,
        (true, b) if b > 900 => 48_000,
        (true, _) => 44_100,
        (false, _) => 44_100,
    }
}

fn format_sample_rate(rate: u32) -> String {
    match rate {
        96_000 => "96 kHz".to_string(),
        88_200 => "88.2 kHz".to_string(),
        48_000 => "48 kHz".to_string(),
        44_100 => "44.1 kHz".to_string(),
        22_050 => "22 kHz".to_string(),
        other => format!("{:.1} kHz", other as f64 / 1000.0),
    }
}

fn swatch(color: u32) -> AnyElement {
    div()
        .w(px(12.0))
        .h(px(12.0))
        .rounded_sm()
        .bg(rgb(color))
        .into_any_element()
}
