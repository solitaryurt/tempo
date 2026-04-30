use serde::Deserialize;

pub(super) const DEFAULT_THEME_ID: &str = "nocturne";

#[derive(Clone)]
pub(super) struct Theme {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) colors: ThemeColors,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct ThemeColors {
    pub(super) app: u32,
    pub(super) panel: u32,
    pub(super) panel_alt: u32,
    pub(super) surface: u32,
    pub(super) elevated: u32,
    pub(super) button: u32,
    pub(super) button_hover: u32,
    pub(super) border: u32,
    pub(super) border_subtle: u32,
    pub(super) border_strong: u32,
    pub(super) text: u32,
    pub(super) text_strong: u32,
    pub(super) text_muted: u32,
    pub(super) text_faint: u32,
    pub(super) accent: u32,
    pub(super) accent_soft: u32,
    pub(super) selected: u32,
    pub(super) playing: u32,
    pub(super) hover: u32,
    pub(super) row: u32,
    pub(super) row_border: u32,
    pub(super) queue: u32,
    pub(super) queue_active: u32,
    pub(super) player: u32,
    pub(super) waveform_bg: u32,
    pub(super) waveform_border: u32,
    pub(super) waveform_line: u32,
    pub(super) waveform_played: u32,
    pub(super) waveform_played_peak: u32,
    pub(super) waveform_idle: u32,
    pub(super) waveform_idle_peak: u32,
    pub(super) waveform_playhead: u32,
    pub(super) transport_primary_bg: u32,
    pub(super) transport_primary_fg: u32,
    pub(super) album_tile_text: u32,
    /// Filled-heart accent for the Liked column / Liked page. Always
    /// reads as a "liked" red regardless of the theme accent so the
    /// affordance stays unambiguous across light/dark themes.
    pub(super) liked: u32,
}

#[derive(Deserialize)]
struct RawTheme {
    id: String,
    name: String,
    description: String,
    colors: RawThemeColors,
}

#[derive(Deserialize)]
struct RawThemeColors {
    app: String,
    panel: String,
    panel_alt: String,
    surface: String,
    elevated: String,
    button: String,
    button_hover: String,
    border: String,
    border_subtle: String,
    border_strong: String,
    text: String,
    text_strong: String,
    text_muted: String,
    text_faint: String,
    accent: String,
    accent_soft: String,
    selected: String,
    playing: String,
    hover: String,
    row: String,
    row_border: String,
    queue: String,
    queue_active: String,
    player: String,
    waveform_bg: String,
    waveform_border: String,
    waveform_line: String,
    waveform_played: String,
    waveform_played_peak: String,
    waveform_idle: String,
    waveform_idle_peak: String,
    waveform_playhead: String,
    transport_primary_bg: String,
    transport_primary_fg: String,
    album_tile_text: String,
    // Renamed from `love` -> `liked`. Old theme YAMLs that still ship
    // `love:` keep working via the serde alias.
    #[serde(alias = "love")]
    liked: String,
}

impl Theme {
    fn from_yaml(contents: &str) -> Result<Self, String> {
        let raw = serde_yaml::from_str::<RawTheme>(contents).map_err(|error| error.to_string())?;
        Ok(Self {
            id: raw.id,
            name: raw.name,
            description: raw.description,
            colors: raw.colors.try_into()?,
        })
    }

    fn fallback() -> Self {
        Self {
            id: DEFAULT_THEME_ID.to_string(),
            name: "Nocturne".to_string(),
            description: "Tempo's original charcoal interface with warm amber highlights."
                .to_string(),
            colors: ThemeColors {
                app: 0x111216,
                panel: 0x15161a,
                panel_alt: 0x17161b,
                surface: 0x131419,
                elevated: 0x1b1c22,
                button: 0x1b1c22,
                button_hover: 0x282a30,
                border: 0x24252b,
                border_subtle: 0x202127,
                border_strong: 0x343741,
                text: 0xd8d8dd,
                text_strong: 0xf0f0f4,
                text_muted: 0x8a8e97,
                text_faint: 0x666a73,
                accent: 0xeeb17d,
                accent_soft: 0xf2c693,
                selected: 0x30323a,
                playing: 0x25262c,
                hover: 0x202229,
                row: 0x131419,
                row_border: 0x202127,
                queue: 0x17161b,
                queue_active: 0x242329,
                player: 0x18191e,
                waveform_bg: 0x111218,
                waveform_border: 0x30323a,
                waveform_line: 0x242833,
                waveform_played: 0x6f9dff,
                waveform_played_peak: 0x9bbdff,
                waveform_idle: 0x383d49,
                waveform_idle_peak: 0x555b69,
                waveform_playhead: 0xd7e5ff,
                transport_primary_bg: 0xe7e7ea,
                transport_primary_fg: 0x111216,
                album_tile_text: 0xf4f0ea,
                liked: 0xe25563,
            },
        }
    }
}

impl TryFrom<RawThemeColors> for ThemeColors {
    type Error = String;

    fn try_from(colors: RawThemeColors) -> Result<Self, Self::Error> {
        Ok(Self {
            app: parse_color(&colors.app)?,
            panel: parse_color(&colors.panel)?,
            panel_alt: parse_color(&colors.panel_alt)?,
            surface: parse_color(&colors.surface)?,
            elevated: parse_color(&colors.elevated)?,
            button: parse_color(&colors.button)?,
            button_hover: parse_color(&colors.button_hover)?,
            border: parse_color(&colors.border)?,
            border_subtle: parse_color(&colors.border_subtle)?,
            border_strong: parse_color(&colors.border_strong)?,
            text: parse_color(&colors.text)?,
            text_strong: parse_color(&colors.text_strong)?,
            text_muted: parse_color(&colors.text_muted)?,
            text_faint: parse_color(&colors.text_faint)?,
            accent: parse_color(&colors.accent)?,
            accent_soft: parse_color(&colors.accent_soft)?,
            selected: parse_color(&colors.selected)?,
            playing: parse_color(&colors.playing)?,
            hover: parse_color(&colors.hover)?,
            row: parse_color(&colors.row)?,
            row_border: parse_color(&colors.row_border)?,
            queue: parse_color(&colors.queue)?,
            queue_active: parse_color(&colors.queue_active)?,
            player: parse_color(&colors.player)?,
            waveform_bg: parse_color(&colors.waveform_bg)?,
            waveform_border: parse_color(&colors.waveform_border)?,
            waveform_line: parse_color(&colors.waveform_line)?,
            waveform_played: parse_color(&colors.waveform_played)?,
            waveform_played_peak: parse_color(&colors.waveform_played_peak)?,
            waveform_idle: parse_color(&colors.waveform_idle)?,
            waveform_idle_peak: parse_color(&colors.waveform_idle_peak)?,
            waveform_playhead: parse_color(&colors.waveform_playhead)?,
            transport_primary_bg: parse_color(&colors.transport_primary_bg)?,
            transport_primary_fg: parse_color(&colors.transport_primary_fg)?,
            album_tile_text: parse_color(&colors.album_tile_text)?,
            liked: parse_color(&colors.liked)?,
        })
    }
}

pub(super) fn default_theme_id() -> String {
    DEFAULT_THEME_ID.to_string()
}

pub(super) fn bundled_themes() -> Vec<Theme> {
    let themes = [
        include_str!("../../themes/nocturne.yaml"),
        include_str!("../../themes/blue-hour.yaml"),
        include_str!("../../themes/signal-green.yaml"),
        include_str!("../../themes/paper-dawn.yaml"),
    ]
    .into_iter()
    .filter_map(|contents| Theme::from_yaml(contents).ok())
    .collect::<Vec<_>>();

    if themes.is_empty() {
        vec![Theme::fallback()]
    } else if themes.iter().any(|theme| theme.id == DEFAULT_THEME_ID) {
        themes
    } else {
        let mut themes = themes;
        themes.insert(0, Theme::fallback());
        themes
    }
}

pub(super) fn resolve_theme_id(theme_id: String, themes: &[Theme]) -> String {
    if themes.iter().any(|theme| theme.id == theme_id) {
        theme_id
    } else {
        DEFAULT_THEME_ID.to_string()
    }
}

fn parse_color(value: &str) -> Result<u32, String> {
    let hex = value
        .trim()
        .trim_start_matches('#')
        .trim_start_matches("0x")
        .trim_start_matches("0X");

    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("invalid color {value:?}; expected #rrggbb"));
    }

    u32::from_str_radix(hex, 16).map_err(|error| error.to_string())
}
