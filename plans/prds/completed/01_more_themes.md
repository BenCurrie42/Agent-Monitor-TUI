# PRD-01: Add 5 more color themes (5 → 10)

## Goal

Expand the theme system from 5 variants to 10 by adding 5 new, visually distinct
themes. The selector in Settings should list all 10 and cycle/select them
exactly as it does today — this is purely additive and must not change the
behavior or appearance of the existing five themes.

New themes to add:

1. **Dracula** — classic purple/pink dark palette.
2. **Gruvbox** — warm retro dark (orange/aqua/yellow on brown-black).
3. **Tokyo Night** — muted blue/indigo dark.
4. **Solarized** — the canonical teal/base03 dark palette.
5. **Paper (Light)** — a light-background theme for bright terminals.

## Acceptance criteria

- [ ] A failing test exists first (TDD).
- [ ] `ThemeVariant::ALL` contains all 10 variants and `ALL.len() == 10`.
- [ ] **`as_u8`/`from_u8` round-trip** holds for every variant
      (`from_u8(v.as_u8()) == v` for all of `ALL`), and the 5 existing variants
      keep their current byte indices (0–4) so a persisted/active selection does
      not silently remap.
- [ ] Each new variant has a unique, non-empty `label()` and a populated
      `Theme::for_variant` arm filling all 8 color slots (`border`, `highlight`,
      `user_badge`, `assistant_badge`, `tool_badge`, `thinking`, `ctx_filled`,
      `ctx_empty`).
- [ ] Every new RGB color uses the `rgb(r, g, b, fallback)` helper with a sensible
      named-color fallback for non-truecolor terminals (no bare `Color::Rgb`).
- [ ] The Settings theme menu renders and selects all 10 entries; cursor
      navigation stays in-bounds (no panic at the new upper index).
- [ ] Existing 5 themes are byte-for-byte unchanged (enum order, indices, labels,
      and color values).
- [ ] Production build passes (`cargo build --release`).
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.

## Out of scope

- Persisting the selected theme across runs (still in-memory only, unless it
  already persists today).
- Reworking the semantic color-slot model or adding new slots.
- Light-mode-specific UI adjustments beyond choosing readable colors for the
  Paper theme — no conditional layout/spacing changes.
- Changing the theme keybindings or Settings view layout.

## Notes

Key file: `src/theme.rs`. The change is mechanical but touches several spots that
must stay in sync — miss one and it won't compile or the menu will be wrong:

- `ThemeVariant` enum — add 5 variants.
- `ALL` array — extend to 10 (this is what the Settings menu iterates).
- `label()` — add arms.
- `as_u8()` — assign indices 5–9 to the new variants; **do not renumber 0–4**.
- `from_u8()` — map 5–9; keep the `_ =>` Coffee default.
- `Theme::for_variant()` — add a fully-populated arm per variant.

Menu bounds: check `app.rs` (`theme_menu_index`, `selected_theme`) and `ui.rs`
for any hard-coded `5`/`< 5` assumptions about the theme count — they should be
driven off `ThemeVariant::ALL.len()`, not a literal.

Palette references (truecolor RGB; pick fallbacks from the named `Color` set):
- Dracula: bg `40,42,54` · pink `255,121,198` · purple `189,147,249` ·
  cyan `139,233,253` · green `80,250,123` · comment `98,114,164`.
- Gruvbox: bg `40,40,40` · orange `254,128,25` · aqua `142,192,124` ·
  yellow `250,189,47` · fg `235,219,178` · gray `146,131,116`.
- Tokyo Night: bg `26,27,38` · blue `122,162,247` · purple `187,154,247` ·
  cyan `125,207,255` · green `158,206,106` · comment `86,95,137`.
- Solarized: base03 `0,43,54` · cyan `42,161,152` · blue `38,139,210` ·
  green `133,153,0` · base0 `131,148,150` · base01 `88,110,117`.
- Paper (light): bg `250,250,247` · accent `30,30,30` · blue `0,95,175` ·
  green `0,135,95` · magenta `135,0,135` · gray `120,120,120`. Pick fallbacks
  that read on a light terminal (avoid `Color::White` for text).
