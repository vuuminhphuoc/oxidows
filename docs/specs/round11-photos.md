# oxi-photos: image viewer/editor with parity to Windows 11 Photos (viewer + Edit & Create)

Rust Cargo workspace (toolchain in rust-toolchain.toml, rustfmt max_width 120). Match existing style: short English
comments that explain why, no unwrap/expect in *-core (`#![forbid(unsafe_code)]`, `#![deny(clippy::unwrap_used,
clippy::expect_used)]`), egui 0.36 via eframe, shared look from `crates/ui-common` (theme, Icon, widgets, menu,
`ui_common::setup`, mica, Updater/About). Do NOT add colors/emoji by hand. Do NOT touch taskmgr/explorer code except
adding small, additive items to ui-common (new Icon variants, app_icon const) and the workspace Cargo.toml member list
(already `apps/*`, `crates/*`) and `[workspace.dependencies]` entries. Other threads are changing ui-common and
explorer at the same time: keep ui-common edits tiny, rebase often.

Clean-room: behaviour is matched by observing Windows Photos and the public docs. Never copy code from WinRust
(no license) or any other project.

## Layout

- `apps/photos` (package `oxi-photos`, bin `oxi-photos.exe`): window, views, input. Icon: add
  `crates/ui-common/assets/photos.ico/.png` (extend scripts/make-icons.py: a picture frame with a mountain, same
  rounded-square style as the other icons), `ui_common::app_icon::PHOTOS_PNG`, `build.rs` like explorer's
  (FileDescription "Photos (oxidows)").
- `crates/photos-core` (pure logic, compiles and unit-tests on Linux): folder list order, zoom/pan math, crop math,
  adjustment math (see Edit), history/undo stack, slideshow timing, `fit_scale`, EXIF-orientation transform table.
- `crates/photos-sys` (Windows API, with a stub for non-Windows so the workspace still builds on Linux): WIC decode
  and encode, metadata, clipboard, print, wallpaper, recycle. Reuse `explorer-sys` for what exists there
  (`read_props`, `shell_open`, recycle/delete, shell icon, `properties`) instead of copying it. Use the `windows`
  crate like explorer-sys does (`Win32_Graphics_Imaging` feature for WIC).

## 1. Opening and decoding (WIC, so every installed codec works)

- `oxi-photos.exe <file>` opens the file and lists its folder (images only, sorted with the same Windows name order
  as Explorer: `explorer_sys::use_windows_name_order` first thing in main). No argument: show an "Open a photo" empty
  state with an Open button (common file dialog, `IFileOpenDialog`, image filter) and also accept drag and drop of a
  file or folder onto the window. Env `OXI_PHOTOS_FILE=<path>` is the same as the argument (CI screenshots).
- Formats: whatever WIC decodes: JPEG, PNG, GIF (animated: play frames with their delays, loop count, disposal), BMP,
  TIFF (multi-page: first page), ICO, and WebP/HEIC/AVIF/camera RAW when the user has the codec. Not SVG (Photos
  does not open it either). Unsupported or corrupt file: centered "Can't open this file" with the file name and an
  Open with... button, never a crash.
- EXIF orientation is applied (`IWICBitmapFlipRotator`/the transform table in photos-core). Metadata via
  `IWICMetadataQueryReader` for date taken, camera make/model, lens, exposure, f-number, ISO, focal length, GPS.
- Speed (the point of the app): decode on a worker thread; show the embedded JPEG thumbnail/preview first when the
  file has one (`GetThumbnail`/`GetPreview`) then swap to the full decode; for a fit-to-window view of a large
  image use `IWICBitmapScaler` to the window size instead of uploading 100 MP. Keep a cache of 3 decoded images
  and prefetch the next and previous file in the background. Hold GPU textures only for the current image.
  `OXI_PHOTOS_TIMING=1` prints open-to-first-pixel ms and full-decode ms to stderr (like the other apps).
- Colour: apply an embedded ICC profile to sRGB through WIC (`IWICColorTransform`) when present; 🟡 acceptable if
  the first version skips it, say so in the parity doc.

## 2. Viewer (matches Photos' single-photo view)

- Fit to window by default; mouse wheel zooms around the cursor (Photos: wheel zooms; hold nothing), Ctrl+wheel
  also zooms, +/- and Ctrl+plus/minus, Ctrl+0 fit, Ctrl+1 actual size (100%), double-click toggles fit / 100%,
  drag to pan when zoomed, trackpad pinch (egui `zoom_delta`), zoom % shown bottom right with a slider and a
  fit button. Zoom range 1% to 6400%; nearest-neighbour above 800% so pixels stay crisp.
- Next/previous: Left/Right arrows, Space/Backspace, PageUp/PageDown, mouse wheel never navigates; hover chevrons
  at the left and right edges; Home/End first/last. The folder list is re-read when the folder changes
  (ReadDirectoryChangesW is overkill: re-list when the window regains focus and after delete/save).
- Filmstrip at the bottom (toggle with a button; hidden in fullscreen and slideshow): 60 px thumbnails from
  `explorer_sys::thumbnail`, loaded lazily in the background, current one highlighted and kept centred.
- Top bar: back-to-nothing is not needed. Left: file name (and "n of m"). Right (icon buttons, Windows 11 look):
  Zoom, Slideshow (F5), Draw/Edit (Ctrl+E), Rotate (Ctrl+R, clockwise 90°, saved to the file on Save), Delete
  (Delete), Info (Ctrl+I), "..." menu.
- "..." menu items: Copy (Ctrl+C, image as bitmap + the file as a file drop in the clipboard), Copy as path
  (Ctrl+Shift+C), Save as... (Ctrl+S when edited, else Ctrl+Shift+S) with the format list from WIC encoders (JPEG,
  PNG, BMP, TIFF, GIF), Print (Ctrl+P: the shell `printto`/`print` verb via ShellExecuteExW, same as Photos hands
  off to the Windows print dialog), Set as > Desktop background / Lock screen is ➖ (lock screen needs WinRT;
  desktop background via `IDesktopWallpaper::SetWallpaper` on the monitor under the window), Open with... (shell
  `openas` verb), Show in folder (select the file in oxi-explorer if installed else explorer.exe /select), Rename
  (F2, in place: a small text field over the file name; keep the extension), Resize (Resize dialog: percent or
  pixels with aspect lock, writes a copy), Properties (Alt+Enter, shell properties dialog), Settings, About.
- Delete: sends to the Recycle Bin (same as Explorer's delete, confirm per the user's shell setting is NOT needed:
  Photos asks; show Photos' dialog "Delete this file? It will be moved to the Recycle Bin." with a "Don't ask again"
  checkbox saved in settings), then moves to the next image (or previous when it was the last). Undo: Ctrl+Z
  restores from the Recycle Bin right after a delete.
- Info pane (Ctrl+I, right side, 320 px, Photos' layout): File name (editable), Date taken, Rating (0-5 stars,
  write `System.Rating`), Description/Tags (editable via `explorer_sys::write_prop` when the file supports it, see
  `can_write_props`), then File: path, date modified, size, dimensions (W x H), then Camera: make, model, f-number,
  exposure, ISO, focal length, flash; Location: latitude/longitude with an "Open in Maps" link (opens
  `https://www.openstreetmap.org/?mlat=..&mlon=..` in the default browser) when GPS exists.
- Fullscreen: F11, Esc leaves; the bar and filmstrip auto-hide after 2 s without mouse movement and return on move.
  Always-on-top is not needed.
- Slideshow (F5): fullscreen, advance every 3 s (settings: 3/5/10/15/30 s), loop, shuffle toggle, Esc stops,
  Left/Right skip, Space pause. A crossfade of 300 ms using ui_common::motion when the user has animations on.
- Window: Mica/transparency exactly like the other apps (`setup_with_transparency`), remembers size/position,
  title "<file name> - Photos". Dark/light follows the system theme setting like the other apps.
- Context menu on the image (right click): the Win11-style menu from ui_common::menu with Copy, Open with, Set as
  desktop background, Rotate, Delete, Show in folder, Properties.
- Drag the image out of the window to another program/Explorer as a file (CF_HDROP): reuse the approach in
  `apps/explorer/src/dnd.rs` / `explorer-sys/src/drop_target.rs`; if that is not trivially reusable, mark it 🟡.
- Touch/pen: ➖.

## 3. Edit (Photos' "Edit image" mode: Crop, Adjust, Filters, Markup; non-destructive until Save)

Ctrl+E toggles. A left/right tool pane like Photos with four tabs; a top bar with Undo/Redo (Ctrl+Z/Ctrl+Shift+Z/
Ctrl+Y), "Revert" ("Reset all"), "Save" split menu: Save a copy (default for read-only/unsupported), Save (overwrite
original; ask once "Save changes to the original?"). Leaving with unsaved edits asks Save / Don't save / Cancel.
Edits run on a downscaled working copy for the preview and are replayed at full size on save (WIC for geometry,
photos-core CPU kernels with rayon-free `std::thread::scope` over rows for colour).

- **Crop**: drag handles on a rule-of-thirds overlay, aspect presets Free, Original, Square, 16:9, 4:3, 3:2, 5:4,
  7:5, 3:4, 2:3 and portrait/landscape toggle, Straighten slider -45..+45° with the grid, Rotate 90° left/right,
  Flip horizontal, Flip vertical. Crop rect math in photos-core with unit tests (aspect lock at corners, clamping,
  straighten inscribed-rect so no empty corners).
- **Adjust**: Light (Brightness, Contrast, Highlights, Shadows), Color (Saturation, Warmth, Tint), plus Clarity/
  Vignette/Red-eye are 🟡 (do Vignette and Auto-enhance; skip Red-eye), "Auto enhance" button (histogram stretch
  + mild contrast), each slider -100..+100 with numeric value and double-click to reset; all pure functions of
  (rgba, params) in photos-core with unit tests on tiny images (identity at 0; monotonic; no overflow at extremes).
- **Filters**: 12 named presets applied as an adjust-parameter set plus a LUT-style tone curve, strength slider
  0..100: Original, Auto, Punch, Clean, Amber, Bright, Soft, Moody, Mono, Noir, Sepia, Fade. Preview tiles are
  small thumbnails rendered from the current image.
- **Markup**: Pens (ballpoint, calligraphy, pencil), highlighter, eraser, 8 colours + custom, size slider; strokes
  stored as vectors (points, colour, width) so Undo works per stroke; rasterised onto the full-size image at save.
  Mouse and pen pressure if egui exposes it, otherwise constant width. Text and shapes are ➖.
- Save: encode with the original container (JPEG quality 90 default, setting; PNG; keep metadata: use
  `IWICBitmapEncoder` + `IWICMetadataBlockWriter`/`CopyMetadata`/in-place metadata writer so EXIF survives, and set
  Orientation to 1 after the rotation is baked in). Overwriting writes to a temp file next to the original then
  replaces it (`ReplaceFileW`) so a failed save never destroys the photo. "Save a copy" names it `<name> (2).<ext>`,
  incrementing like Explorer.

## 4. Settings and housekeeping

- Settings page (gear in the bar, like Explorer's Options): Theme (System/Light/Dark, `ui_common::Theme`), Slideshow
  speed and shuffle default, JPEG quality, "Ask before deleting", "Show filmstrip", Always show zoom control,
  Default for images ("Set oxi-photos as default image viewer" / Restore) reusing `ui_common::default_app`
  and the same HKCU-only mechanism as Explorer/Task Manager (see docs, PR #10). Do NOT write the registry in tests
  or CI, and do not add any code that runs it without the user clicking the button. About section via
  `ui_common::Updater` like the other apps (add the app name in the same list the other apps use).
- Settings file `photos.ini` next to the other apps' ini files (follow `explorer`'s settings.rs for location and
  format).
- Single instance per window is NOT required (Photos opens one window per file launch from Explorer); but opening
  a second image from Explorer may just open a second window.
- Keyboard map documented at the top of apps/photos/README.md; docs/parity/photos.md written in the same format as
  docs/parity/explorer.md (Feature | Status | Notes; ✅ done, 🟡 partial, ❌ not yet, ➖ out of scope with reason).
  Honest statuses only: anything not run on a real Windows window is 🟡 "live GUI not yet verified".

## 5. Tests (CI is Windows, `cargo test --workspace`)

- photos-core unit tests as listed above (zoom math keeps the point under the cursor fixed; fit_scale; orientation
  transforms for all 8 EXIF values; crop; adjustments; undo stack; GIF frame timing; folder ordering).
- photos-sys integration tests on Windows (`#[cfg(windows)]`): generate PNG/JPEG/GIF/BMP/TIFF in a temp dir with the
  WIC encoder (no binary fixtures except the one below), decode back and compare pixel/size; EXIF orientation
  roundtrip; encode with metadata keeps `System.Photo.CameraModel`; save-over via ReplaceFileW leaves the original
  intact on a forced failure; WIC scaler output size; animated GIF has N frames with delays.
- Oracle test (like docs/testing.md): decode the same files with our WIC path and with `IShellItemImageFactory`
  (the Explorer thumbnail at 256 px) and compare dimensions and mean colour within a small tolerance.
- `apps/photos/testdata/sample.jpg`: small (< 60 KB) generated image (gradient + shapes), committed, created by a
  script `scripts/make-testdata.py`. Add `apps/photos/smoke.json`:
  `{"views": [{"name": "viewer", "settle": 4, "env": {"OXI_PHOTOS_FILE": "apps/photos/testdata/sample.jpg"}},
  {"name": "edit", "settle": 4, "env": {"OXI_PHOTOS_FILE": "apps/photos/testdata/sample.jpg", "OXI_PHOTOS_EDIT": "1"}},
  {"name": "info", "settle": 4, "env": {"OXI_PHOTOS_FILE": "apps/photos/testdata/sample.jpg", "OXI_PHOTOS_INFO": "1"}}]}`
  so smoke-all.ps1 screenshots each. The plain launch with no argument must show a window within 30 s (that is how
  smoke.ps1 starts every exe).
- Bench: photos-sys bench (criterion-free like explorer's vs_windows.rs): time-to-decode a generated 24 MP JPEG
  through our path (embedded preview first / scaled) versus the full-size WIC decode; table to the job summary;
  fail only on a tenfold slowdown.

## 6. Rules for the work

- Branch `omp/photos` in a new worktree from current main (`..\oxidows-photos`). One logical commit per section is
  fine. Do not bump the version. Do not touch `.github/workflows` unless the smoke or tests need a new step.
- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` (Linux clippy is fine for the
  non-Windows parts; Windows code is checked by CI). Do not skip or disable any test.
- Review traps seen in earlier rounds (Claude will check each): mixing screen and content coordinates in
  `show_viewport`; opening COM stores/decoders per frame instead of once per image; deleting blank lines/comments
  in main.rs; duplicating shared helpers that already exist in ui-common/explorer-sys; blocking the UI thread with
  decode, thumbnails or file listing; forgetting `com_init()` on each worker thread.
