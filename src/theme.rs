use ratatui::style::Color;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    Coffee,
    NordicFrost,
    ForestMoss,
    Cyberpunk,
    DefaultDark,
}

impl ThemeVariant {
    pub const ALL: [ThemeVariant; 5] = [
        ThemeVariant::Coffee,
        ThemeVariant::NordicFrost,
        ThemeVariant::ForestMoss,
        ThemeVariant::Cyberpunk,
        ThemeVariant::DefaultDark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeVariant::Coffee => "Coffee (Espresso & Crema)",
            ThemeVariant::NordicFrost => "Nordic Frost",
            ThemeVariant::ForestMoss => "Forest Moss",
            ThemeVariant::Cyberpunk => "Cyberpunk Neon",
            ThemeVariant::DefaultDark => "Default Dark",
        }
    }
}

/// Semantic color slots. UI code references these by purpose (border,
/// highlight, badge for each role) rather than reaching for concrete RGB
/// values — that's what makes the swap atomic.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub border: Color,
    pub highlight: Color,
    pub user_badge: Color,
    pub assistant_badge: Color,
    pub tool_badge: Color,
    pub thinking: Color,
    pub ctx_filled: Color,
    pub ctx_empty: Color,
}

fn truecolor() -> bool {
    matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor") | Ok("24bit")
    )
}

/// RGB when the terminal advertises truecolor, otherwise the named fallback.
fn rgb(r: u8, g: u8, b: u8, fb: Color) -> Color {
    if truecolor() {
        Color::Rgb(r, g, b)
    } else {
        fb
    }
}

impl Theme {
    pub fn for_variant(v: ThemeVariant) -> Self {
        match v {
            ThemeVariant::Coffee => Theme {
                border:          rgb(46, 37, 37,    Color::DarkGray),
                highlight:       rgb(212, 163, 115, Color::Yellow),
                user_badge:      rgb(127, 106, 85,  Color::Green),
                assistant_badge: rgb(156, 102, 68,  Color::Magenta),
                tool_badge:      rgb(237, 224, 212, Color::White),
                thinking:        rgb(92, 77, 77,    Color::Indexed(240)),
                ctx_filled:      rgb(212, 163, 115, Color::Yellow),
                ctx_empty:       rgb(46, 37, 37,    Color::DarkGray),
            },
            ThemeVariant::NordicFrost => Theme {
                border:          rgb(46, 52, 64,    Color::DarkGray),
                highlight:       rgb(136, 192, 208, Color::Cyan),
                user_badge:      rgb(216, 222, 233, Color::White),
                assistant_badge: rgb(129, 161, 193, Color::Blue),
                tool_badge:      rgb(163, 190, 140, Color::Green),
                thinking:        rgb(76, 86, 106,   Color::Indexed(240)),
                ctx_filled:      rgb(136, 192, 208, Color::Cyan),
                ctx_empty:       rgb(46, 52, 64,    Color::DarkGray),
            },
            ThemeVariant::ForestMoss => Theme {
                border:          rgb(27, 46, 36,    Color::DarkGray),
                highlight:       rgb(229, 169, 59,  Color::Yellow),
                user_badge:      rgb(143, 188, 143, Color::Green),
                assistant_badge: rgb(46, 139, 87,   Color::Green),
                tool_badge:      rgb(245, 245, 220, Color::White),
                thinking:        rgb(63, 94, 77,    Color::Indexed(240)),
                ctx_filled:      rgb(229, 169, 59,  Color::Yellow),
                ctx_empty:       rgb(27, 46, 36,    Color::DarkGray),
            },
            ThemeVariant::Cyberpunk => Theme {
                border:          rgb(26, 16, 60,    Color::DarkGray),
                highlight:       rgb(255, 0, 127,   Color::Magenta),
                user_badge:      rgb(0, 243, 255,   Color::Cyan),
                assistant_badge: rgb(157, 0, 255,   Color::Magenta),
                tool_badge:      rgb(57, 255, 20,   Color::Green),
                thinking:        rgb(75, 0, 130,    Color::Indexed(240)),
                ctx_filled:      rgb(255, 0, 127,   Color::Magenta),
                ctx_empty:       rgb(26, 16, 60,    Color::DarkGray),
            },
            ThemeVariant::DefaultDark => Theme {
                border:          Color::DarkGray,
                highlight:       Color::Yellow,
                user_badge:      Color::Green,
                assistant_badge: Color::Magenta,
                tool_badge:      Color::White,
                thinking:        Color::Indexed(240),
                ctx_filled:      Color::Yellow,
                ctx_empty:       Color::DarkGray,
            },
        }
    }
}

static CURRENT: OnceLock<RwLock<Theme>> = OnceLock::new();

fn cell() -> &'static RwLock<Theme> {
    CURRENT.get_or_init(|| RwLock::new(Theme::for_variant(ThemeVariant::Coffee)))
}

/// Returns a snapshot of the currently active theme. Cheap (Copy) so callers
/// can read individual fields multiple times without holding the lock.
pub fn current() -> Theme {
    *cell().read().expect("theme lock poisoned")
}

pub fn set(variant: ThemeVariant) {
    *cell().write().expect("theme lock poisoned") = Theme::for_variant(variant);
}
