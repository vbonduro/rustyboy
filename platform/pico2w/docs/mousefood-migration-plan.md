# Migrate menus + loading screens to ratatui via mousefood

## Context

The firmware's menu/loading UI is currently a **hand-rolled, per-row pixel renderer**
(`display/menu.rs`, `display/loading.rs`, `display/text.rs`, `display/font.rs`) that
streams 480-byte rows straight to the ILI9341 over async SPI from
`display/hw.rs::GameDisplay`. It bypasses embedded-graphics entirely (raw
CASET/RASET/RAMWR commands) and carries bespoke optimizations: frame-hash skip,
single-item/text-only partial repaints, anti-tearing draw order, a 2× 8×8 palette
font, crash `!` badge, `*` marker, and **marquee scrolling**.

We want **richer UI capability** for upcoming screens (scrollbars, tables, styled
layouts, popups) by adopting ratatui via the **mousefood** embedded-graphics backend.
A visual change from the current pixel-precise design is acceptable. Scope: **menus +
loading screens**. Splash animation and the 60 fps game-frame DMA path stay on the
existing custom code. Marquee scrolling for long ROM names is preserved via a custom
ratatui widget.

The big enabler: menu **logic** is already cleanly separated from rendering. `menu.rs`
(state machines, `MenuInput`/`MenuEffect`, `MenuFrame` descriptor, fully unit-tested)
is reused unchanged. This is a **presentation-layer rewrite only**.

mousefood targets embedded-graphics **0.8.2** (matches our `0.8`) and ratatui 0.30 /
no_std + alloc. We already run no_std with `embedded-alloc` (160 KiB heap).

## Key design decision: framebuffer-backed DrawTarget

mousefood needs a synchronous `DrawTarget<Color = Rgb565>`. A naive raw-SPI DrawTarget
would set a 1-pixel window per glyph pixel (`draw_iter`) — tens of thousands of blocking
window-sets per repaint, far too slow. Instead:

- Render into an in-RAM **`&'static mut` Rgb565 framebuffer** (the DrawTarget is just a
  memory write — fast), tracking a **dirty bounding box** (min/max y written).
- After `terminal.draw(...)`, **flush only the dirty rows** to the panel using the
  existing async SPI DMA in `GameDisplay` (reuse `set_window` + `self.spi.write`).
- A **persistent ratatui `Terminal`** diffs cells across draws, so steady-state repaints
  (e.g. marquee) touch only a few cells → a small dirty band → cheap DMA flush. This
  replaces the hand-rolled hash-skip and `draw_menu_item`/`draw_menu_item_text` partials.

**SRAM budget is the #1 risk — verify before building UI** (see Verification). A full
240×320×2 framebuffer is 153.6 KiB. Mitigation: menus and the 101 KiB
`CORE0_SCALE_BUF` (game-frame prescale) are **never live at the same time**, so back
both with **one shared `static_cell` region** sized for the framebuffer (net new SRAM
≈ 52 KiB, not 153 KiB). If even that doesn't fit alongside the 160 KiB heap + stacks,
fall back to flushing the terminal in horizontal bands with a smaller band buffer.

## Implementation

### 1. Dependencies (ARM-only block in `Cargo.toml`)
- `mousefood` with `default-features = false` (drop the alloc-heavy `fonts`/unicodefonts
  feature; supply an embedded-graphics `MonoFont` via `EmbeddedBackendConfig` instead).
- `ratatui` `default-features = false`, features `["portable-atomic"]`; `ratatui-core`.
- Keep host (non-ARM) builds compiling: gate the new UI module behind
  `#[cfg(target_arch = "arm")]` like `display/hw.rs`, OR pull ratatui host-side too for
  the render unit tests (preferred — see Testing).
- **Verify at this step:** `cargo build --release` for `thumbv8m.main-none-eabihf` links,
  and check binary size / a clean `cargo tree` (no accidental std).

### 2. Framebuffer DrawTarget adapter — new `display/eg_target.rs`
- `struct FbTarget { buf: &'static mut [Rgb565; 240*320], dirty: Option<(u16,u16)> }`.
- Impl `DrawTarget<Color = Rgb565>` + `OriginDimensions` (240×320). `draw_iter` and
  `fill_contiguous` write into `buf` and widen the dirty y-range.
- `fn take_dirty(&mut self) -> Option<RenderWindow>` returns and clears the band.
- Back `buf` with a shared `static_cell` region unioned with `CORE0_SCALE_BUF` (see SRAM
  note). Reuse existing `RenderWindow` (`display/mod.rs`) for the flush window type.

### 3. UI host object — new `display/ui.rs` (the ratatui layer)
- Owns the persistent `Terminal<EmbeddedBackend<FbTarget>>` + `EmbeddedBackendConfig`
  (MonoFont, ColorTheme mapping DMG palette C0–C3).
- `render_menu(frame: &MenuFrame, f: &mut ratatui::Frame)`: maps the existing `MenuFrame`
  descriptor to widgets — `Block` title (header + crash `!` badge), a `List` with
  highlight symbol `>`, dimmed style for `enabled[i] == false`, the `*` marker span, and
  a footer line (`A:SELECT  B:BACK`). Long selected items use the **marquee widget**.
- `render_loading(frame: &LoadingFrame, f)`: title/filename + a gauge for progress
  (replaces `display/loading.rs` row renderer).
- Custom **`MarqueeLine` widget**: scrolls the selected item's text using `marquee_frame`
  (already on `MenuFrame`); it must always write its cells (so ratatui's diff re-emits
  them) → those cells land in the dirty band each animation tick.

### 4. Wire into `GameDisplay` — keep call sites stable
- Embed the `Ui` (terminal + `FbTarget`) inside `GameDisplay` (or a sibling held next to
  it). Reimplement the existing public methods so **no state file changes**:
  - `draw_menu(&MenuFrame)` → `terminal.draw(|f| render_menu(frame, f))` then flush
    `take_dirty()` band via DMA.
  - `draw_loading_progress` / `draw_loading_bar` → `render_loading`.
  - `draw_menu_item` / `draw_menu_item_text` become **thin wrappers over `draw_menu`**
    (cell-diff handles partial updates) — or delete them and update the few `rom_list.rs`
    callers. Marquee tick (`tick_marquee` in `state/rom_list.rs`) keeps calling a redraw;
    diffing keeps it cheap.
- `draw_letterbox_bars` still resets caches; with ratatui, force a full repaint by
  clearing the terminal's prev-buffer on the next menu open (so stale game pixels under
  the menu are repainted). Keep the existing reset of `prev_frame_hash`.

### 5. Remove/replace the old renderer
- Delete `display/menu.rs`, `display/loading.rs`, `display/text.rs`, `display/font.rs`
  and their re-exports in `display/mod.rs` once `ui.rs` covers their surfaces.
- Keep `menu.rs` (logic) and `display/mod.rs` palette constants / `Display<D>` / scaling
  / splash untouched. `MenuFrame`, `LoadingFrame`, `menu_item_needs_marquee` stay as the
  descriptor API consumed by `ui.rs`.

## Critical files
- `platform/pico2w/Cargo.toml` — add deps (ARM block).
- `platform/pico2w/src/display/hw.rs` — `GameDisplay`: embed `Ui`, reimplement
  `draw_menu*` / `draw_loading*`, share the framebuffer with `CORE0_SCALE_BUF`.
- `platform/pico2w/src/display/eg_target.rs` *(new)* — `FbTarget` DrawTarget + dirty box.
- `platform/pico2w/src/display/ui.rs` *(new)* — ratatui Terminal, `render_menu`,
  `render_loading`, `MarqueeLine` widget.
- `platform/pico2w/src/display/mod.rs` — module wiring; drop old re-exports.
- Unchanged: `src/menu.rs`, `src/state/*.rs` (call sites preserved), splash, game DMA.

## Testing
- **Logic:** `menu.rs` unit tests stay green (no changes).
- **Render (host):** pull ratatui on the host build and assert with a ratatui
  `TestBackend`/`Buffer` that `render_menu`/`render_loading` produce the expected cells
  (cursor `>`, dimmed disabled item, `*` marker, crash badge, footer, marquee offset) —
  replacing the deleted `display/menu.rs`/`text.rs` pixel tests.
- Drop the old pixel-level tests in the deleted modules.

## Verification (end-to-end)
1. **SRAM gate (do first):** after adding the shared framebuffer static, build release
   and check the linker fits (`cargo size --release -- -A`, or map file) — confirm
   `.bss` + stacks + 160 KiB heap + 153 KiB fb region ≤ 520 KiB. If not, switch to banded
   flush before proceeding.
2. `cd platform/pico2w && cargo build --release` (target `thumbv8m.main-none-eabihf`).
3. Flash + RTT: `pkill -f probe-rs; sleep 1` then `cargo run --release` (per memory:
   kill stale probe-rs first; runner is `rb-flash`).
4. On hardware, exercise each surface: main menu (with/without staged ROM), in-game
   pause (LOAD greyed when no save), settings, wifi menu, ROM list paging, **marquee on a
   long ROM name**, crash `!` badge, and a ROM **loading progress** screen. Confirm no
   tearing on cursor moves and acceptable repaint latency.
5. Confirm game-frame path and splash are visually unchanged (untouched code).
