# Themes

Last reviewed: 2026-11-22.

Nocturne has 14 themes: **light and dark**, each with **seven accents**:

| Accent | Light accent | Dark accent | Surfaces |
|---|---|---|---|
| Default | Nocturne ultramarine | starlight periwinkle | starlight / midnight-navy |
| Red | crimson | salmon | clean charcoal / tinted red paper |
| Green | forest green | mint | clean charcoal / tinted green paper |
| Yellow | dark amber | warm yellow | clean charcoal / tinted yellow paper |
| Blue | cobalt blue | sky | clean charcoal / tinted blue paper |
| Gray | slate | light slate | true neutral alabaster / charcoal |
| Purple | purple | lighter purple | clean charcoal / tinted purple paper |

`Default` preserves Nocturne's signature starlight (light) and midnight-navy (dark) surfaces.
`Gray` uses true neutral alabaster and charcoal surfaces with no navy bias. Chromatic accents
(`Red`, `Green`, `Yellow`, `Blue`, `Purple`) blend cleanly from neutral paper and dark charcoal
so warm and cool dark hues never muddy into navy.

## The switcher

The title band, at the top right beside Search, has a **Theme: Light Default** button. It opens a grid of 14 swatches (a light row
and a dark row, seven accents each) and a switch, **Follow the system light or dark setting**.
Each swatch shows the theme's surface colour ringed and dotted in its accent. A swatch names a
mode, so clicking one picks that mode and turns the switch off. The switch is the way back to
following the system. (Until 2026-10-17 a swatch changed only the accent while the switch was
on, so picking "Dark Default" on a light desktop did nothing visible. That was a bug.)

The choice is saved in the settings file (`theme_mode`: `light`, `dark` or `system`;
`theme_accent`) outside any notebook, and is restored at the next start.

## How colours, radii, and elevation are made

`crates/ui-egui/src/theme.rs`. Each theme builds a `Tokens` set from its accent's base surfaces
(light or dark), the accent colour, and a tint amount. The UI uses tokens only; there are no
hard-coded UI colours in chrome, menus, modals, or tool racks.

### Semantic surface, border, and button tokens

- **Surfaces**: `panel`, `surface_elevated` (modals, popovers, menus, floating cards), `editor`
  (page paper), `faint` (subtle recessed fills), `code_bg`, `band` (top title band).
- **Borders & separators**: `border` (visible structural border for inputs, modals, popups,
  buttons), `border_subtle` (quiet hairline for dividers and group separators), `container_border`,
  `table_line`.
- **Text & emphasis**: `text`, `strong`, `muted`, `disabled` (quieter than `muted` while still
  legible), `accent`, `accent_text` (WCAG AA accent text on panels and elevated surfaces), `link`,
  `ok`, `warn`.
- **Action buttons**: `btn_primary_bg`, `btn_primary_hover`, `btn_primary_text` and
  `btn_danger_bg`, `btn_danger_hover`, `btn_danger_text`. Light themes use deep fills with white
  text (darkening on hover); dark themes use luminous accent/coral fills with near-black ink
  (brightening on hover) so button labels never fall below WCAG AA in any dark accent.
- **Modals & overlays**: `scrim` (dimmed backdrop), `close_hover` (destructive close-button hover
  fill with white glyph), `shadow_floating()`, and `shadow_popup()`.
- **Unified corner radii**: `RADIUS_CONTROL` (`4`), `RADIUS_POPUP` (`8`), `RADIUS_CARD` (`10`),
  `RADIUS_MODAL` (`12`), and `RADIUS_PILL` (`99`).
- **Full `Theme::visuals()`**: populates all five egui widget states (`noninteractive`, `inactive`,
  `hovered`, `active`, `open`), selection wash (`row_selected`), text cursor, window/popup fills,
  strokes, corner radii, and shadows.

### WCAG contrast guarantees

Contrast is part of the construction, not a hope. The tokens are adjusted until:

| Pair | Target (WCAG contrast ratio) |
|---|---|
| Body text on `panel`, `surface_elevated`, `editor`, or `faint` | 7 to 1 |
| Strong text on `panel` or `surface_elevated` | 7 to 1 |
| Text on a hovered or selected tree row (`row_hover`, `row_selected`, `selection`) | 7 to 1 |
| Secondary (`muted`) text on `panel` or `surface_elevated` | 4.5 to 1 |
| Accent text (`accent_text`) on `panel` or `surface_elevated` | 4.5 to 1 |
| Primary & danger button text on rest and hover fills (`btn_primary_*`, `btn_danger_*`) | 4.5 to 1 |
| White close glyph on `close_hover` | 4.5 to 1 |
| Links on the editor (`link`) and warning text (`warn`) | 4.5 to 1 |
| Structural borders (`border`), disabled icons/text (`disabled`), accent, ribbon marks, folder/section/ok icons | 3 to 1 |
| Primary & danger button fills against `surface_elevated` | 3 to 1 |
| Subtle hairlines (`border_subtle`) on `panel` or `surface_elevated` | 1.5 to 1 |
| Rule lines on a page (`rule_line`) | 1.25 to 1, and at most 4.5 to 1 |
| Grid and margin lines (`rule_line_strong`) | 1.8 to 1, and at most 4.5 to 1 |

The last two are page decoration, not text, so they have an upper bound as well: a rule line
must be visible on the page but must never compete with the writing on top of it. Text keeps
4.5 to 1 against a rule line it crosses.

The unit tests `every_theme_is_readable`, `visuals_follow_the_tokens`,
`gray_is_true_neutral_and_default_preserves_midnight_navy`, and
`every_mark_meets_wcag_non_text_contrast_in_all_fourteen_themes` verify all 14 themes.

## Adding or changing a theme

- A new accent: add it to `Accent`, to `Accent::ALL`, to `id`, `label`, `base`, `tint`, and
  `base_surfaces`, and run the tests. The contrast tests tell you if any colour pair is
  unreadable. Regenerate the theme screenshots if you want one for it.
- Changing a colour: edit `theme.rs`, run `cargo test -p nocturne-ui-egui`, look at the diffs
  of the screenshot tests, and accept them if intended (see `testing.md`).

## How this compares with the apps people come from

What follows comes from web searches (support pages and articles), not from any
specification. The Office note taking app on Windows offers an Office theme (Colorful, Dark
Gray, Black, White) in File, Options, General, and its Windows 10 version has a light, dark or
follow-Windows setting under Settings and Options. Notebooks and sections can be coloured, and a page can have a
background colour (View, Page Color). Nocturne's accents are a per-app setting; per-notebook
and per-section colours are not built yet (see `gaps.md`).
