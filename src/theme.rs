use ratatui::style::Color;

use crate::parse::Level;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub border: Color,
    pub border_focused: Color,
    pub title: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub error: Color,
    pub warn: Color,
    pub info: Color,
    pub debug: Color,
    pub trace: Color,
    pub accent: Color,
    pub badge: Color,
    pub status_bar_fg: Color,
    pub status_bar_bg: Color,
    pub fuzzy_match: Color,
    pub text: Color,
    pub text_dim: Color,
    pub bg: Color,
    pub modal_border: Color,
    pub modal_bg: Color,
    pub modal_title: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    pub header_accent: Color,
    pub rate_bar: Color,
    pub rate_bar_bg: Color,
    pub menu_hover: Color,
    pub divider: Color,
    pub success: Color,
    // Trend arrow colors
    pub trend_up: Color,
    pub trend_down: Color,
    pub trend_stable: Color,
    // Severity badge backgrounds
    pub badge_error_bg: Color,
    pub badge_warn_bg: Color,
    pub badge_info_bg: Color,
    pub badge_debug_bg: Color,
    // Count heat levels
    pub count_hot: Color,
    pub count_warm: Color,
    pub count_cold: Color,
    // Sparkline
    pub sparkline: Color,
    pub sparkline_dim: Color,
    // Banner / wordmark
    pub banner_primary: Color,
    pub banner_accent: Color,
    pub banner_tagline: Color,
    pub banner_separator: Color,
}

impl Theme {
    /// Ordered list of all available themes.
    #[allow(dead_code)]
    pub fn all() -> Vec<Theme> {
        vec![
            Self::matrix(),
            Self::nebula(),
            Self::frostbyte(),
            Self::ember(),
            Self::deepwave(),
            Self::signal(),
            Self::obsidian(),
            Self::mono(),
        ]
    }

    /// All theme names in cycle order.
    pub fn all_names() -> Vec<&'static str> {
        vec![
            "matrix", "nebula", "frostbyte", "ember", "deepwave", "signal", "obsidian", "mono",
        ]
    }

    /// Look up a theme by name. Returns None for unknown names.
    pub fn by_name(name: &str) -> Option<Theme> {
        match name {
            "matrix" => Some(Self::matrix()),
            "nebula" => Some(Self::nebula()),
            "frostbyte" => Some(Self::frostbyte()),
            "ember" => Some(Self::ember()),
            "deepwave" => Some(Self::deepwave()),
            "signal" => Some(Self::signal()),
            "obsidian" => Some(Self::obsidian()),
            "mono" => Some(Self::mono()),
            _ => None,
        }
    }

    /// Return the next theme in the cycle.
    pub fn next(&self) -> Theme {
        let names = Self::all_names();
        let idx = names.iter().position(|&n| n == self.name).unwrap_or(0);
        let next_name = names[(idx + 1) % names.len()];
        Self::by_name(next_name).unwrap()
    }

    // ── matrix ─────────────────────────────────────────────────────
    // Mood: Radar console / subtle Matrix aesthetic
    pub fn matrix() -> Self {
        Theme {
            name: "matrix".into(),
            border: Color::Rgb(0, 80, 0),
            border_focused: Color::Rgb(0, 200, 80),
            title: Color::Rgb(0, 220, 100),
            selected_fg: Color::Rgb(0, 0, 0),
            selected_bg: Color::Rgb(0, 220, 100),
            error: Color::Rgb(255, 85, 85),
            warn: Color::Rgb(255, 184, 108),
            info: Color::Rgb(0, 255, 120),
            debug: Color::Rgb(0, 180, 80),
            trace: Color::Rgb(0, 110, 45),
            accent: Color::Rgb(0, 255, 120),
            badge: Color::Rgb(0, 220, 100),
            status_bar_fg: Color::Rgb(0, 220, 100),
            status_bar_bg: Color::Rgb(0, 30, 0),
            fuzzy_match: Color::Rgb(180, 255, 100),
            text: Color::Rgb(0, 220, 100),
            text_dim: Color::Rgb(0, 140, 60),
            bg: Color::Reset,
            modal_border: Color::Rgb(0, 200, 80),
            modal_bg: Color::Rgb(0, 10, 0),
            modal_title: Color::Rgb(0, 255, 120),
            header_bg: Color::Rgb(0, 20, 0),
            header_fg: Color::Rgb(0, 180, 80),
            header_accent: Color::Rgb(0, 255, 120),
            rate_bar: Color::Rgb(0, 255, 120),
            rate_bar_bg: Color::Rgb(0, 30, 0),
            menu_hover: Color::Rgb(0, 40, 0),
            divider: Color::Rgb(0, 80, 0),
            success: Color::Rgb(0, 255, 120),
            trend_up: Color::Rgb(0, 255, 120),
            trend_down: Color::Rgb(255, 85, 85),
            trend_stable: Color::Rgb(0, 120, 50),
            badge_error_bg: Color::Rgb(80, 20, 20),
            badge_warn_bg: Color::Rgb(80, 55, 20),
            badge_info_bg: Color::Rgb(0, 50, 20),
            badge_debug_bg: Color::Rgb(0, 35, 15),
            count_hot: Color::Rgb(255, 85, 85),
            count_warm: Color::Rgb(255, 184, 108),
            count_cold: Color::Rgb(0, 120, 50),
            sparkline: Color::Rgb(0, 220, 100),
            sparkline_dim: Color::Rgb(0, 130, 60),
            banner_primary: Color::Rgb(0, 160, 0),
            banner_accent: Color::Rgb(0, 255, 120),
            banner_tagline: Color::Rgb(0, 140, 60),
            banner_separator: Color::Rgb(0, 120, 0),
        }
    }

    // ── nebula ─────────────────────────────────────────────────────
    // Mood: Deep space observability — soft purples, electric cyan
    pub fn nebula() -> Self {
        Theme {
            name: "nebula".into(),
            border: Color::Rgb(75, 65, 110),
            border_focused: Color::Rgb(0, 220, 255),
            title: Color::Rgb(200, 180, 255),
            selected_fg: Color::Rgb(15, 12, 30),
            selected_bg: Color::Rgb(0, 220, 255),
            error: Color::Rgb(255, 100, 100),
            warn: Color::Rgb(255, 190, 80),
            info: Color::Rgb(100, 220, 255),
            debug: Color::Rgb(170, 140, 220),
            trace: Color::Rgb(75, 65, 110),
            accent: Color::Rgb(0, 220, 255),
            badge: Color::Rgb(0, 220, 255),
            status_bar_fg: Color::Rgb(200, 180, 255),
            status_bar_bg: Color::Rgb(25, 20, 45),
            fuzzy_match: Color::Rgb(255, 220, 100),
            text: Color::Rgb(200, 190, 230),
            text_dim: Color::Rgb(100, 95, 130),
            bg: Color::Reset,
            modal_border: Color::Rgb(130, 100, 200),
            modal_bg: Color::Rgb(20, 15, 35),
            modal_title: Color::Rgb(0, 220, 255),
            header_bg: Color::Rgb(22, 18, 38),
            header_fg: Color::Rgb(180, 165, 220),
            header_accent: Color::Rgb(0, 220, 255),
            rate_bar: Color::Rgb(0, 220, 255),
            rate_bar_bg: Color::Rgb(35, 30, 55),
            menu_hover: Color::Rgb(40, 35, 65),
            divider: Color::Rgb(75, 65, 110),
            success: Color::Rgb(100, 230, 200),
            trend_up: Color::Rgb(100, 230, 200),
            trend_down: Color::Rgb(255, 100, 100),
            trend_stable: Color::Rgb(100, 95, 130),
            badge_error_bg: Color::Rgb(90, 25, 25),
            badge_warn_bg: Color::Rgb(90, 65, 20),
            badge_info_bg: Color::Rgb(15, 60, 75),
            badge_debug_bg: Color::Rgb(50, 35, 80),
            count_hot: Color::Rgb(255, 100, 100),
            count_warm: Color::Rgb(255, 190, 80),
            count_cold: Color::Rgb(100, 95, 130),
            sparkline: Color::Rgb(0, 200, 240),
            sparkline_dim: Color::Rgb(40, 90, 120),
            banner_primary: Color::Rgb(150, 120, 210),
            banner_accent: Color::Rgb(0, 220, 255),
            banner_tagline: Color::Rgb(100, 180, 220),
            banner_separator: Color::Rgb(75, 55, 120),
        }
    }

    // ── frostbyte ──────────────────────────────────────────────────
    // Mood: Clean, icy enterprise — cool blues and silver
    pub fn frostbyte() -> Self {
        Theme {
            name: "frostbyte".into(),
            border: Color::Rgb(70, 85, 110),
            border_focused: Color::Rgb(100, 200, 255),
            title: Color::Rgb(220, 230, 245),
            selected_fg: Color::Rgb(15, 20, 35),
            selected_bg: Color::Rgb(100, 200, 255),
            error: Color::Rgb(220, 60, 60),
            warn: Color::Rgb(240, 180, 70),
            info: Color::Rgb(120, 200, 240),
            debug: Color::Rgb(150, 160, 210),
            trace: Color::Rgb(80, 95, 115),
            accent: Color::Rgb(100, 200, 255),
            badge: Color::Rgb(100, 200, 255),
            status_bar_fg: Color::Rgb(210, 220, 235),
            status_bar_bg: Color::Rgb(20, 28, 42),
            fuzzy_match: Color::Rgb(255, 230, 100),
            text: Color::Rgb(210, 220, 235),
            text_dim: Color::Rgb(100, 115, 140),
            bg: Color::Reset,
            modal_border: Color::Rgb(100, 200, 255),
            modal_bg: Color::Rgb(15, 20, 32),
            modal_title: Color::Rgb(100, 200, 255),
            header_bg: Color::Rgb(18, 24, 38),
            header_fg: Color::Rgb(190, 200, 220),
            header_accent: Color::Rgb(100, 200, 255),
            rate_bar: Color::Rgb(100, 200, 255),
            rate_bar_bg: Color::Rgb(30, 40, 55),
            menu_hover: Color::Rgb(35, 45, 65),
            divider: Color::Rgb(70, 85, 110),
            success: Color::Rgb(100, 210, 180),
            trend_up: Color::Rgb(100, 210, 180),
            trend_down: Color::Rgb(220, 60, 60),
            trend_stable: Color::Rgb(100, 115, 140),
            badge_error_bg: Color::Rgb(85, 20, 20),
            badge_warn_bg: Color::Rgb(85, 60, 15),
            badge_info_bg: Color::Rgb(20, 55, 75),
            badge_debug_bg: Color::Rgb(40, 40, 75),
            count_hot: Color::Rgb(220, 60, 60),
            count_warm: Color::Rgb(240, 180, 70),
            count_cold: Color::Rgb(100, 115, 140),
            sparkline: Color::Rgb(100, 200, 255),
            sparkline_dim: Color::Rgb(50, 100, 130),
            banner_primary: Color::Rgb(120, 160, 210),
            banner_accent: Color::Rgb(100, 210, 255),
            banner_tagline: Color::Rgb(100, 170, 220),
            banner_separator: Color::Rgb(60, 80, 110),
        }
    }

    // ── ember ──────────────────────────────────────────────────────
    // Mood: High-alert / warm signal — burnt orange, gold
    pub fn ember() -> Self {
        Theme {
            name: "ember".into(),
            border: Color::Rgb(100, 75, 50),
            border_focused: Color::Rgb(255, 200, 50),
            title: Color::Rgb(245, 230, 210),
            selected_fg: Color::Rgb(25, 18, 10),
            selected_bg: Color::Rgb(255, 200, 50),
            error: Color::Rgb(255, 70, 60),
            warn: Color::Rgb(240, 160, 40),
            info: Color::Rgb(180, 220, 80),
            debug: Color::Rgb(200, 160, 130),
            trace: Color::Rgb(100, 85, 70),
            accent: Color::Rgb(255, 200, 50),
            badge: Color::Rgb(255, 200, 50),
            status_bar_fg: Color::Rgb(240, 225, 200),
            status_bar_bg: Color::Rgb(35, 25, 15),
            fuzzy_match: Color::Rgb(255, 255, 120),
            text: Color::Rgb(240, 225, 200),
            text_dim: Color::Rgb(130, 115, 95),
            bg: Color::Reset,
            modal_border: Color::Rgb(255, 200, 50),
            modal_bg: Color::Rgb(28, 20, 12),
            modal_title: Color::Rgb(255, 200, 50),
            header_bg: Color::Rgb(30, 22, 14),
            header_fg: Color::Rgb(220, 195, 160),
            header_accent: Color::Rgb(255, 200, 50),
            rate_bar: Color::Rgb(240, 160, 50),
            rate_bar_bg: Color::Rgb(50, 38, 22),
            menu_hover: Color::Rgb(55, 42, 28),
            divider: Color::Rgb(100, 75, 50),
            success: Color::Rgb(180, 220, 80),
            trend_up: Color::Rgb(180, 220, 80),
            trend_down: Color::Rgb(255, 70, 60),
            trend_stable: Color::Rgb(130, 115, 95),
            badge_error_bg: Color::Rgb(100, 25, 20),
            badge_warn_bg: Color::Rgb(95, 60, 10),
            badge_info_bg: Color::Rgb(50, 70, 18),
            badge_debug_bg: Color::Rgb(65, 48, 35),
            count_hot: Color::Rgb(255, 70, 60),
            count_warm: Color::Rgb(240, 160, 40),
            count_cold: Color::Rgb(130, 115, 95),
            sparkline: Color::Rgb(240, 150, 50),
            sparkline_dim: Color::Rgb(120, 80, 30),
            banner_primary: Color::Rgb(210, 140, 50),
            banner_accent: Color::Rgb(255, 200, 60),
            banner_tagline: Color::Rgb(200, 160, 80),
            banner_separator: Color::Rgb(120, 85, 40),
        }
    }

    // ── deepwave ───────────────────────────────────────────────────
    // Mood: Ocean analytics — teal, aqua, deep navy
    pub fn deepwave() -> Self {
        Theme {
            name: "deepwave".into(),
            border: Color::Rgb(50, 80, 90),
            border_focused: Color::Rgb(0, 230, 200),
            title: Color::Rgb(210, 235, 235),
            selected_fg: Color::Rgb(10, 20, 25),
            selected_bg: Color::Rgb(0, 230, 200),
            error: Color::Rgb(255, 110, 90),
            warn: Color::Rgb(240, 190, 80),
            info: Color::Rgb(80, 220, 200),
            debug: Color::Rgb(140, 160, 210),
            trace: Color::Rgb(60, 80, 95),
            accent: Color::Rgb(0, 230, 200),
            badge: Color::Rgb(0, 230, 200),
            status_bar_fg: Color::Rgb(200, 225, 225),
            status_bar_bg: Color::Rgb(15, 28, 35),
            fuzzy_match: Color::Rgb(255, 240, 100),
            text: Color::Rgb(200, 225, 225),
            text_dim: Color::Rgb(90, 115, 125),
            bg: Color::Reset,
            modal_border: Color::Rgb(0, 200, 180),
            modal_bg: Color::Rgb(12, 22, 28),
            modal_title: Color::Rgb(0, 230, 200),
            header_bg: Color::Rgb(14, 25, 32),
            header_fg: Color::Rgb(170, 200, 200),
            header_accent: Color::Rgb(0, 230, 200),
            rate_bar: Color::Rgb(0, 220, 190),
            rate_bar_bg: Color::Rgb(25, 45, 55),
            menu_hover: Color::Rgb(30, 50, 60),
            divider: Color::Rgb(50, 80, 90),
            success: Color::Rgb(80, 230, 180),
            trend_up: Color::Rgb(80, 230, 180),
            trend_down: Color::Rgb(255, 110, 90),
            trend_stable: Color::Rgb(90, 115, 125),
            badge_error_bg: Color::Rgb(90, 30, 25),
            badge_warn_bg: Color::Rgb(85, 65, 20),
            badge_info_bg: Color::Rgb(15, 60, 55),
            badge_debug_bg: Color::Rgb(35, 40, 70),
            count_hot: Color::Rgb(255, 110, 90),
            count_warm: Color::Rgb(240, 190, 80),
            count_cold: Color::Rgb(90, 115, 125),
            sparkline: Color::Rgb(0, 210, 185),
            sparkline_dim: Color::Rgb(0, 100, 90),
            banner_primary: Color::Rgb(0, 170, 150),
            banner_accent: Color::Rgb(0, 240, 210),
            banner_tagline: Color::Rgb(60, 180, 165),
            banner_separator: Color::Rgb(40, 100, 95),
        }
    }

    // ── signal ─────────────────────────────────────────────────────
    // Mood: Minimalist professional — clean grays, single subtle accent
    pub fn signal() -> Self {
        Theme {
            name: "signal".into(),
            border: Color::Rgb(65, 65, 70),
            border_focused: Color::Rgb(110, 160, 220),
            title: Color::Rgb(210, 210, 215),
            selected_fg: Color::Rgb(15, 15, 18),
            selected_bg: Color::Rgb(110, 160, 220),
            error: Color::Rgb(230, 80, 80),
            warn: Color::Rgb(220, 170, 60),
            info: Color::Rgb(120, 190, 120),
            debug: Color::Rgb(130, 150, 190),
            trace: Color::Rgb(70, 70, 75),
            accent: Color::Rgb(110, 160, 220),
            badge: Color::Rgb(110, 160, 220),
            status_bar_fg: Color::Rgb(200, 200, 205),
            status_bar_bg: Color::Rgb(22, 22, 26),
            fuzzy_match: Color::Rgb(240, 220, 100),
            text: Color::Rgb(200, 200, 205),
            text_dim: Color::Rgb(95, 95, 100),
            bg: Color::Reset,
            modal_border: Color::Rgb(110, 160, 220),
            modal_bg: Color::Rgb(18, 18, 22),
            modal_title: Color::Rgb(110, 160, 220),
            header_bg: Color::Rgb(20, 20, 24),
            header_fg: Color::Rgb(180, 180, 185),
            header_accent: Color::Rgb(110, 160, 220),
            rate_bar: Color::Rgb(110, 160, 220),
            rate_bar_bg: Color::Rgb(35, 35, 40),
            menu_hover: Color::Rgb(38, 38, 44),
            divider: Color::Rgb(65, 65, 70),
            success: Color::Rgb(120, 190, 120),
            trend_up: Color::Rgb(120, 190, 120),
            trend_down: Color::Rgb(230, 80, 80),
            trend_stable: Color::Rgb(95, 95, 100),
            badge_error_bg: Color::Rgb(75, 22, 22),
            badge_warn_bg: Color::Rgb(70, 52, 15),
            badge_info_bg: Color::Rgb(25, 55, 25),
            badge_debug_bg: Color::Rgb(35, 40, 58),
            count_hot: Color::Rgb(230, 80, 80),
            count_warm: Color::Rgb(220, 170, 60),
            count_cold: Color::Rgb(95, 95, 100),
            sparkline: Color::Rgb(110, 160, 220),
            sparkline_dim: Color::Rgb(55, 80, 110),
            banner_primary: Color::Rgb(140, 140, 150),
            banner_accent: Color::Rgb(120, 170, 230),
            banner_tagline: Color::Rgb(110, 140, 190),
            banner_separator: Color::Rgb(55, 55, 60),
        }
    }

    // ── obsidian ───────────────────────────────────────────────────
    // Mood: Ultra minimal grayscale with strategic accent pops
    pub fn obsidian() -> Self {
        Theme {
            name: "obsidian".into(),
            border: Color::Rgb(55, 55, 58),
            border_focused: Color::Rgb(80, 200, 160),
            title: Color::Rgb(190, 190, 195),
            selected_fg: Color::Rgb(10, 10, 12),
            selected_bg: Color::Rgb(80, 200, 160),
            error: Color::Rgb(220, 75, 75),
            warn: Color::Rgb(200, 160, 60),
            info: Color::Rgb(140, 180, 140),
            debug: Color::Rgb(130, 130, 145),
            trace: Color::Rgb(60, 60, 65),
            accent: Color::Rgb(80, 200, 160),
            badge: Color::Rgb(80, 200, 160),
            status_bar_fg: Color::Rgb(180, 180, 185),
            status_bar_bg: Color::Rgb(18, 18, 20),
            fuzzy_match: Color::Rgb(80, 220, 170),
            text: Color::Rgb(185, 185, 190),
            text_dim: Color::Rgb(85, 85, 90),
            bg: Color::Reset,
            modal_border: Color::Rgb(80, 200, 160),
            modal_bg: Color::Rgb(15, 15, 18),
            modal_title: Color::Rgb(80, 200, 160),
            header_bg: Color::Rgb(16, 16, 19),
            header_fg: Color::Rgb(165, 165, 170),
            header_accent: Color::Rgb(80, 200, 160),
            rate_bar: Color::Rgb(80, 200, 160),
            rate_bar_bg: Color::Rgb(32, 32, 36),
            menu_hover: Color::Rgb(35, 35, 40),
            divider: Color::Rgb(55, 55, 58),
            success: Color::Rgb(80, 200, 160),
            trend_up: Color::Rgb(80, 200, 160),
            trend_down: Color::Rgb(220, 75, 75),
            trend_stable: Color::Rgb(85, 85, 90),
            badge_error_bg: Color::Rgb(70, 20, 20),
            badge_warn_bg: Color::Rgb(65, 50, 15),
            badge_info_bg: Color::Rgb(30, 52, 30),
            badge_debug_bg: Color::Rgb(35, 35, 42),
            count_hot: Color::Rgb(220, 75, 75),
            count_warm: Color::Rgb(200, 160, 60),
            count_cold: Color::Rgb(85, 85, 90),
            sparkline: Color::Rgb(160, 160, 165),
            sparkline_dim: Color::Rgb(70, 70, 75),
            banner_primary: Color::Rgb(130, 130, 135),
            banner_accent: Color::Rgb(80, 210, 165),
            banner_tagline: Color::Rgb(80, 170, 140),
            banner_separator: Color::Rgb(50, 50, 55),
        }
    }

    // ── mono ───────────────────────────────────────────────────────
    // Pure grayscale — no color, maximum readability
    pub fn mono() -> Self {
        Theme {
            name: "mono".into(),
            border: Color::Rgb(88, 88, 88),
            border_focused: Color::Rgb(200, 200, 200),
            title: Color::Rgb(200, 200, 200),
            selected_fg: Color::Rgb(0, 0, 0),
            selected_bg: Color::Rgb(200, 200, 200),
            error: Color::Rgb(200, 200, 200),
            warn: Color::Rgb(200, 200, 200),
            info: Color::Rgb(160, 160, 160),
            debug: Color::Rgb(120, 120, 120),
            trace: Color::Rgb(88, 88, 88),
            accent: Color::Rgb(200, 200, 200),
            badge: Color::Rgb(200, 200, 200),
            status_bar_fg: Color::Rgb(0, 0, 0),
            status_bar_bg: Color::Rgb(160, 160, 160),
            fuzzy_match: Color::Rgb(200, 200, 200),
            text: Color::Rgb(200, 200, 200),
            text_dim: Color::Rgb(88, 88, 88),
            bg: Color::Reset,
            modal_border: Color::Rgb(200, 200, 200),
            modal_bg: Color::Rgb(20, 20, 20),
            modal_title: Color::Rgb(200, 200, 200),
            header_bg: Color::Rgb(40, 40, 40),
            header_fg: Color::Rgb(200, 200, 200),
            header_accent: Color::Rgb(200, 200, 200),
            rate_bar: Color::Rgb(200, 200, 200),
            rate_bar_bg: Color::Rgb(60, 60, 60),
            menu_hover: Color::Rgb(60, 60, 60),
            divider: Color::Rgb(88, 88, 88),
            success: Color::Rgb(200, 200, 200),
            trend_up: Color::Rgb(200, 200, 200),
            trend_down: Color::Rgb(160, 160, 160),
            trend_stable: Color::Rgb(88, 88, 88),
            badge_error_bg: Color::Rgb(60, 60, 60),
            badge_warn_bg: Color::Rgb(50, 50, 50),
            badge_info_bg: Color::Rgb(40, 40, 40),
            badge_debug_bg: Color::Rgb(35, 35, 35),
            count_hot: Color::Rgb(200, 200, 200),
            count_warm: Color::Rgb(160, 160, 160),
            count_cold: Color::Rgb(88, 88, 88),
            sparkline: Color::Rgb(200, 200, 200),
            sparkline_dim: Color::Rgb(100, 100, 100),
            banner_primary: Color::Rgb(160, 160, 160),
            banner_accent: Color::Rgb(200, 200, 200),
            banner_tagline: Color::Rgb(120, 120, 120),
            banner_separator: Color::Rgb(88, 88, 88),
        }
    }

    pub fn level_color(&self, level: Level) -> Color {
        match level {
            Level::Error => self.error,
            Level::Warn => self.warn,
            Level::Info => self.info,
            Level::Debug => self.debug,
            Level::Trace => self.trace,
            Level::Unknown => self.text_dim,
        }
    }

    pub fn badge_bg(&self, level: Level) -> Color {
        match level {
            Level::Error => self.badge_error_bg,
            Level::Warn => self.badge_warn_bg,
            Level::Info => self.badge_info_bg,
            Level::Debug => self.badge_debug_bg,
            Level::Trace | Level::Unknown => self.bg,
        }
    }

    pub fn trend_color(&self, trend: crate::pattern::Trend) -> Color {
        match trend {
            crate::pattern::Trend::Up => self.trend_up,
            crate::pattern::Trend::Down => self.trend_down,
            crate::pattern::Trend::Stable => self.trend_stable,
        }
    }

    pub fn count_color(&self, rate_1m: f64) -> Color {
        if rate_1m > 10.0 {
            self.count_hot
        } else if rate_1m > 2.0 {
            self.count_warm
        } else {
            self.count_cold
        }
    }
}
