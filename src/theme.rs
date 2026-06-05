use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    Coffee,
    NordicFrost,
    ForestMoss,
    Cyberpunk,
    DefaultDark,
    Dracula,
    Gruvbox,
    TokyoNight,
    Solarized,
    Paper,
}

impl ThemeVariant {
    pub const ALL: [ThemeVariant; 10] = [
        ThemeVariant::Coffee,
        ThemeVariant::NordicFrost,
        ThemeVariant::ForestMoss,
        ThemeVariant::Cyberpunk,
        ThemeVariant::DefaultDark,
        ThemeVariant::Dracula,
        ThemeVariant::Gruvbox,
        ThemeVariant::TokyoNight,
        ThemeVariant::Solarized,
        ThemeVariant::Paper,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeVariant::Coffee => "Coffee (Espresso & Crema)",
            ThemeVariant::NordicFrost => "Nordic Frost",
            ThemeVariant::ForestMoss => "Forest Moss",
            ThemeVariant::Cyberpunk => "Cyberpunk Neon",
            ThemeVariant::DefaultDark => "Default Dark",
            ThemeVariant::Dracula => "Dracula",
            ThemeVariant::Gruvbox => "Gruvbox",
            ThemeVariant::TokyoNight => "Tokyo Night",
            ThemeVariant::Solarized => "Solarized",
            ThemeVariant::Paper => "Paper (Light)",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            ThemeVariant::Coffee => 0,
            ThemeVariant::NordicFrost => 1,
            ThemeVariant::ForestMoss => 2,
            ThemeVariant::Cyberpunk => 3,
            ThemeVariant::DefaultDark => 4,
            ThemeVariant::Dracula => 5,
            ThemeVariant::Gruvbox => 6,
            ThemeVariant::TokyoNight => 7,
            ThemeVariant::Solarized => 8,
            ThemeVariant::Paper => 9,
        }
    }

    fn from_u8(b: u8) -> Self {
        match b {
            1 => ThemeVariant::NordicFrost,
            2 => ThemeVariant::ForestMoss,
            3 => ThemeVariant::Cyberpunk,
            4 => ThemeVariant::DefaultDark,
            5 => ThemeVariant::Dracula,
            6 => ThemeVariant::Gruvbox,
            7 => ThemeVariant::TokyoNight,
            8 => ThemeVariant::Solarized,
            9 => ThemeVariant::Paper,
            _ => ThemeVariant::Coffee,
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
    // Cached at first call: COLORTERM doesn't change during a session and
    // env::var is otherwise hit on every Color resolution × every frame.
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor") | Ok("24bit")
        )
    })
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
                border: rgb(46, 37, 37, Color::DarkGray),
                highlight: rgb(212, 163, 115, Color::Yellow),
                user_badge: rgb(127, 106, 85, Color::Green),
                assistant_badge: rgb(156, 102, 68, Color::Magenta),
                tool_badge: rgb(237, 224, 212, Color::White),
                thinking: rgb(92, 77, 77, Color::Indexed(240)),
                ctx_filled: rgb(212, 163, 115, Color::Yellow),
                ctx_empty: rgb(46, 37, 37, Color::DarkGray),
            },
            ThemeVariant::NordicFrost => Theme {
                border: rgb(46, 52, 64, Color::DarkGray),
                highlight: rgb(136, 192, 208, Color::Cyan),
                user_badge: rgb(216, 222, 233, Color::White),
                assistant_badge: rgb(129, 161, 193, Color::Blue),
                tool_badge: rgb(163, 190, 140, Color::Green),
                thinking: rgb(76, 86, 106, Color::Indexed(240)),
                ctx_filled: rgb(136, 192, 208, Color::Cyan),
                ctx_empty: rgb(46, 52, 64, Color::DarkGray),
            },
            ThemeVariant::ForestMoss => Theme {
                border: rgb(27, 46, 36, Color::DarkGray),
                highlight: rgb(229, 169, 59, Color::Yellow),
                user_badge: rgb(143, 188, 143, Color::Green),
                assistant_badge: rgb(46, 139, 87, Color::Green),
                tool_badge: rgb(245, 245, 220, Color::White),
                thinking: rgb(63, 94, 77, Color::Indexed(240)),
                ctx_filled: rgb(229, 169, 59, Color::Yellow),
                ctx_empty: rgb(27, 46, 36, Color::DarkGray),
            },
            ThemeVariant::Cyberpunk => Theme {
                border: rgb(26, 16, 60, Color::DarkGray),
                highlight: rgb(255, 0, 127, Color::Magenta),
                user_badge: rgb(0, 243, 255, Color::Cyan),
                assistant_badge: rgb(157, 0, 255, Color::Magenta),
                tool_badge: rgb(57, 255, 20, Color::Green),
                thinking: rgb(75, 0, 130, Color::Indexed(240)),
                ctx_filled: rgb(255, 0, 127, Color::Magenta),
                ctx_empty: rgb(26, 16, 60, Color::DarkGray),
            },
            ThemeVariant::DefaultDark => Theme {
                border: Color::DarkGray,
                highlight: Color::Yellow,
                user_badge: Color::Green,
                assistant_badge: Color::Magenta,
                tool_badge: Color::White,
                thinking: Color::Indexed(240),
                ctx_filled: Color::Yellow,
                ctx_empty: Color::DarkGray,
            },
            // Dracula: purple/pink dark palette.
            // bg 40,42,54 · pink 255,121,198 · purple 189,147,249
            // cyan 139,233,253 · green 80,250,123 · comment 98,114,164
            ThemeVariant::Dracula => Theme {
                border: rgb(40, 42, 54, Color::DarkGray),
                highlight: rgb(255, 121, 198, Color::Magenta),
                user_badge: rgb(80, 250, 123, Color::Green),
                assistant_badge: rgb(189, 147, 249, Color::Magenta),
                tool_badge: rgb(139, 233, 253, Color::Cyan),
                thinking: rgb(98, 114, 164, Color::Indexed(240)),
                ctx_filled: rgb(255, 121, 198, Color::Magenta),
                ctx_empty: rgb(40, 42, 54, Color::DarkGray),
            },
            // Gruvbox: warm retro dark (orange/aqua/yellow on brown-black).
            // bg 40,40,40 · orange 254,128,25 · aqua 142,192,124
            // yellow 250,189,47 · fg 235,219,178 · gray 146,131,116
            ThemeVariant::Gruvbox => Theme {
                border: rgb(40, 40, 40, Color::DarkGray),
                highlight: rgb(254, 128, 25, Color::Yellow),
                user_badge: rgb(142, 192, 124, Color::Green),
                assistant_badge: rgb(250, 189, 47, Color::Yellow),
                tool_badge: rgb(235, 219, 178, Color::White),
                thinking: rgb(146, 131, 116, Color::Indexed(240)),
                ctx_filled: rgb(254, 128, 25, Color::Yellow),
                ctx_empty: rgb(40, 40, 40, Color::DarkGray),
            },
            // Tokyo Night: muted blue/indigo dark.
            // bg 26,27,38 · blue 122,162,247 · purple 187,154,247
            // cyan 125,207,255 · green 158,206,106 · comment 86,95,137
            ThemeVariant::TokyoNight => Theme {
                border: rgb(26, 27, 38, Color::DarkGray),
                highlight: rgb(122, 162, 247, Color::Blue),
                user_badge: rgb(158, 206, 106, Color::Green),
                assistant_badge: rgb(187, 154, 247, Color::Magenta),
                tool_badge: rgb(125, 207, 255, Color::Cyan),
                thinking: rgb(86, 95, 137, Color::Indexed(240)),
                ctx_filled: rgb(122, 162, 247, Color::Blue),
                ctx_empty: rgb(26, 27, 38, Color::DarkGray),
            },
            // Solarized: canonical teal/base03 dark palette.
            // base03 0,43,54 · cyan 42,161,152 · blue 38,139,210
            // green 133,153,0 · base0 131,148,150 · base01 88,110,117
            ThemeVariant::Solarized => Theme {
                border: rgb(0, 43, 54, Color::DarkGray),
                highlight: rgb(42, 161, 152, Color::Cyan),
                user_badge: rgb(133, 153, 0, Color::Green),
                assistant_badge: rgb(38, 139, 210, Color::Blue),
                tool_badge: rgb(131, 148, 150, Color::White),
                thinking: rgb(88, 110, 117, Color::Indexed(240)),
                ctx_filled: rgb(42, 161, 152, Color::Cyan),
                ctx_empty: rgb(0, 43, 54, Color::DarkGray),
            },
            // Paper (light): light-background theme for bright terminals.
            // bg 250,250,247 · accent 30,30,30 · blue 0,95,175
            // green 0,135,95 · magenta 135,0,135 · gray 120,120,120
            ThemeVariant::Paper => Theme {
                border: rgb(120, 120, 120, Color::DarkGray),
                highlight: rgb(0, 95, 175, Color::Blue),
                user_badge: rgb(0, 135, 95, Color::Green),
                assistant_badge: rgb(135, 0, 135, Color::Magenta),
                tool_badge: rgb(30, 30, 30, Color::Black),
                thinking: rgb(120, 120, 120, Color::DarkGray),
                ctx_filled: rgb(0, 95, 175, Color::Blue),
                ctx_empty: rgb(120, 120, 120, Color::DarkGray),
            },
        }
    }
}

// Theme storage is a single atomic byte: the variant index. Reads in
// `current()` reconstruct the 8-slot Theme on demand — cheaper than a RwLock
// since `c_*()` helpers fire dozens of times per frame and the rebuild is
// just a match returning Copy fields. Cached truecolor() makes each rebuild
// branch-only with no syscalls.
static CURRENT_VARIANT: AtomicU8 = AtomicU8::new(0);

/// Returns a snapshot of the currently active theme. Lock-free; safe to call
/// hundreds of times per frame.
pub fn current() -> Theme {
    Theme::for_variant(ThemeVariant::from_u8(
        CURRENT_VARIANT.load(Ordering::Relaxed),
    ))
}

pub fn set(variant: ThemeVariant) {
    CURRENT_VARIANT.store(variant.as_u8(), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_contains_10_variants() {
        assert_eq!(ThemeVariant::ALL.len(), 10);
    }

    #[test]
    fn as_u8_from_u8_round_trip() {
        for v in &ThemeVariant::ALL {
            assert_eq!(ThemeVariant::from_u8(v.as_u8()), *v);
        }
    }

    #[test]
    fn existing_variants_keep_byte_indices() {
        assert_eq!(ThemeVariant::Coffee.as_u8(), 0);
        assert_eq!(ThemeVariant::NordicFrost.as_u8(), 1);
        assert_eq!(ThemeVariant::ForestMoss.as_u8(), 2);
        assert_eq!(ThemeVariant::Cyberpunk.as_u8(), 3);
        assert_eq!(ThemeVariant::DefaultDark.as_u8(), 4);
    }

    #[test]
    fn new_variants_have_indices_5_to_9() {
        assert_eq!(ThemeVariant::Dracula.as_u8(), 5);
        assert_eq!(ThemeVariant::Gruvbox.as_u8(), 6);
        assert_eq!(ThemeVariant::TokyoNight.as_u8(), 7);
        assert_eq!(ThemeVariant::Solarized.as_u8(), 8);
        assert_eq!(ThemeVariant::Paper.as_u8(), 9);
    }

    #[test]
    fn all_variants_have_non_empty_labels() {
        for v in &ThemeVariant::ALL {
            assert!(!v.label().is_empty(), "{v:?} has empty label");
        }
        // Uniqueness
        let labels: Vec<&str> = ThemeVariant::ALL.iter().map(|v| v.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len(), "duplicate theme labels");
    }

    #[test]
    fn all_variants_have_populated_theme() {
        // Just constructing `for_variant` for each new variant must not panic.
        for v in &ThemeVariant::ALL {
            let _ = Theme::for_variant(*v);
        }
    }
}
