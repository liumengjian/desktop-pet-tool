# desktop-pet-tool

English | [中文](README.md)

A **Tauri** desktop floating ball: it turns online file links into files you can paste straight into a chat, and converts / compresses documents you drop onto it, right on the spot.
The installer is about **10.4 MB** (it uses the system WebView2, with no bundled browser engine; most of the size comes from the embedded Chinese font subset).

## Features

### 1. The floating ball

- **Off-white round ball**: drag it anywhere on the desktop, always on top, expressions rendered live by the emotion-ball engine (SVG + plain JS, zero dependencies)
- **Left click**: wake up / dismiss the bubble that is currently open. **Left click no longer opens the input box**; the paste bubble is opened only from the right-click menu item "Copy documents"
- **Right-click menu** (two sections):
  - Documents: **Copy documents** / **Document tools**
  - Photos: **Face matching** / **Remove watermark**
  - Temp folder and app: **Open temp folder** / **Clean temp folder** / **Settings** / **Launch at login** / **Quit**
- **Expressions** (32 emotion-ball faces, expression only — no mouth):
  - Built-in **pool of 27 random expressions**: every 1.2–4.2 s one is shown, then it falls back to "idle blank stare"
  - Fixed triggers: copy succeeded → happy; copy failed → sad; three quick clicks → angry (3 s); a file dragged onto the ball → **searching for material** (emotion-ball 40, kept until the document panel closes); a document job running → permanently angry (so random faces do not interrupt); 1 min 50 s idle → sleepy (10 s); 2 min idle → asleep (zzz drifting at the top right), and a click/drag wakes it through the sleepy transition
  - Eyes **follow the mouse across the whole screen** (engine gaze looks at the cursor; the gaze is fixed while angry)

### 2. Copy documents

Right-click "Copy documents" → paste a direct file URL (http/https, follows redirects up to 10 hops) → press Enter → the file is downloaded and written to the clipboard as a standard `CF_HDROP`, so Ctrl+V in WeChat / DingTalk / WeCom sends the file itself.

### 3. Document tools (inside the bubble, no separate window)

Drop files onto the ball and a chat bubble appears next to it; the whole flow never leaves the ball:

1. **Drop** → the bubble lists the pending files (remove one by one, or add more)
2. **Two action buttons**: `Convert` / `Compress`
3. "Convert" reveals the **target format** chips; "Compress" reveals **three strength levels** (Smaller / Standard / Sharper)
4. "Start" → a **progress ring around the ball plus a percentage pill under it**, while the bubble shows the current file and percentage
5. On completion a **result bubble** appears: the size change of every file plus an "Open temp folder" button
6. If you dismiss the bubble mid-run, the job keeps going and the progress stays on the ball

All outputs land in the **temp folder** (default `%TEMP%\desktop-pet-tool`), the same folder as the files downloaded by "Copy documents", so "Clean temp folder" wipes everything at once.

### 4. Face matching

Right-click "Face matching" opens the same kind of bubble, with **two upload groups**. It pairs the people in the two groups one-to-one and produces a single Word file:

1. **Two upload areas**: `Group 1` / `Group 2`, each taking several photos (remove one by one, or add more); supported formats are `png / jpg / jpeg / bmp / tif / webp`, and non-images are rejected with a message
2. "Start" → the **progress ring around the ball plus a percentage pill**, while the bubble shows the current stage (preparing / downloading det_10g.onnx / Group 1 photo5.png / writing Word)
3. On completion a result bubble appears: how many pairs were found, how many were left unmatched, the Word path and an "Open temp folder" button

Matching rules:

- **One-to-one across the two groups**: each photo in group 1 pairs with at most one photo of group 2 and vice versa, so no photo is reused; pairing is a globally optimal assignment, not per-file maximum, and low-similarity combinations are left unpaired
- **Face only**: detect → five-point alignment → embedding → cosine similarity. **No OCR**, so printed names or ID numbers on the visa photo play no part
- **Local CPU inference**: InsightFace ONNX — SCRFD `det_10g` for detection, ResNet50 `w600k_r50` for embeddings; thread count is `min(4, CPU-1)`, and a few dozen photos usually take ten-odd seconds

Model delivery:

- Models and the onnxruntime dependencies are **downloaded on first use** into `%LOCALAPPDATA%\com.desktop-pet-tool.float\face\v1\` (`models\` + `deps\` + `marker.json`, about 233 MB) and work offline afterwards
- The base URL sits in `BASE_URL` in `src-tauri\src\face\models.rs` (currently `.../tools/desktop-pet-tool/face-models/pack`); the manifest and sha256 values are documented in `tools\face-models\README.md`. Whether anything is downloaded depends only on the local files being complete and their sha256 matching — `marker.json` merely records the cached version for troubleshooting
- A failed download exits with the reason instead of dying halfway

Word layout:

- One Word file (`人脸比对.docx`, written to the temp folder) with **exactly one pair per page**: group 1 on the left, group 2 on the right, centred side by side, each image at most 7.5 cm wide and 10.3 cm tall
- Pages carry **no heading, no table, no file name, no similarity text** — just the two photos
- When anything is left unmatched, the **last page** lists it in plain text: `共配对 N 组。第一组 未匹配 x 张，第二组 未匹配 y 张，无法自动处理 z 张`, followed by file name plus best score or the skip reason. If everything matched, that page does not exist

Thresholds:

- Whether a pair may be formed is decided by `--low` (default **0.35**); anything below it is never assigned
- Status wording and the manual-review count use `--high` (default **0.45**)
- Both live as argparse defaults in `src-tauri\binaries\pdfconv\scripts\face_match.py`, and Rust never passes them, so **changing the defaults is global**. In dev the script is read from the repo, so a rerun picks it up without a rebuild
- Measured on real samples: impostors peak around 0.11 while true pairs score 0.55–0.78, so the defaults leave a wide margin and rarely need tuning

Small faces:

- The detection viewport is fixed at 640 (SCRFD is trained at that scale; raising it actually hurts recall). When a face's short side is below 112 px, the face is cropped with a 2.5× expanded box, upscaled to a 640 frame, re-detected for landmarks, and aligned plus embedded from that upscaled image
- Only a short side below 24 px counts as "face too small" and gets skipped; pairs below 112 px still form normally and are merely flagged as `lowq` in the JSON payload (the Word page shows no text)

### 5. Remove watermark

Right-click "Remove watermark" opens the same kind of bubble. Pick an image, paint or box the watermark on the canvas, and a local model fills in the selected area, producing a new PNG:

1. **Pick an image**: supported formats are `png / jpg / jpeg / bmp / tif / webp`, non-images are rejected with a message. The canvas fits the image on first load; **scroll the wheel while the cursor is over the canvas to zoom in / out (0.4–4×)**, with the image layer and the mask layer scaled together
2. **Two ways to mark the area**: a **Brush** (adjustable size) or a **Box** rectangle; strokes can be undone or cleared, and their coordinates are mapped back to the original image by the current display scale
3. Click "Remove" → the **progress ring around the ball plus a percentage pill**, while the bubble shows the current stage (preparing model / inferring)
4. On completion a result bubble shows the output path plus an "Open temp folder" button; the output lands in the shared temp folder

Inpainting notes:

- **Local CPU inference**: a LaMa ONNX inpainting model (`inpainting_lama.onnx`, about 88 MB); the onnxruntime runtime is reused from face matching, so it is not downloaded twice
- **Models are downloaded on first use** into `%LOCALAPPDATA%\com.desktop-pet-tool.float\watermark\v1\` (`models\` + `deps\` + `marker.json`) and work offline afterwards; deps reuse `face\v1\deps` first, and fall back to downloading the same ort package only when the face copy is absent
- Pipeline: strokes are redrawn into a mask → dilated twice plus Gaussian feathering → inferred at 512×512 → only the mask region is blended back with the feathered alpha, keeping the original image outside the mask so edges stay soft
- The manifest and sha256 values are documented in `deploy\watermark-models\README.md`; you can also place the onnx manually into `models\` for offline use

## Supported formats and conversion matrix

| Input | Can convert to | Compressible |
|---|---|---|
| Image `png/jpg/jpeg/bmp/gif/tif/tiff/webp` | jpg, png, bmp, tif, pdf | ✅ |
| Word `docx` | txt, html, md, pdf | ✅ |
| Word legacy `doc` (OLE CFB) | txt, md, html, docx, pdf | ❌ (convert to docx first) |
| `rtf` / `odt` / `epub` | txt, md, html, docx, pdf | ❌ |
| Sheet `xlsx` | csv, txt, html, pdf | ✅ |
| Sheet `ods` | csv, xlsx, txt, html, docx, pdf | ❌ |
| Slides `pptx` | txt, html, md, pdf | ✅ |
| Slides `odp` | txt, md, html, docx, pdf | ❌ |
| `pdf` | txt, md, html, docx, xlsx, pptx, jpg, png | ✅ |
| `txt` | docx, html, pdf | — nothing to compress |
| `md` | docx, html, txt, pdf | — nothing to compress |
| `html/htm` | txt, md, pdf | — nothing to compress |
| `csv` | xlsx, txt, html, pdf | — nothing to compress |
| Legacy `xls` / `ppt` | — | — not supported yet |

- Hard limit of **2 GB per file**; files above it never enter the list
- With several files the target formats are intersected; when the intersection is empty the union is used and each file is reported at run time
- Compression levels: images by longest edge and quality, Office by repacking and stripping optional parts, PDF by streaming rewrite plus image resampling

## Language

The UI is available in **Chinese (default)** and **English**:

- Switch it in **Settings → Language / 语言**; the change applies immediately and is saved without pressing Save
- The whole main-process side follows: right-click menu, tray menu, system dialog titles, document job prompts
- The front end follows too: paste bubble, document bubble, face-matching bubble, watermark bubble, result rows, settings page
- The choice is stored in `config.json` as `lang` (`zh` | `en`); old config files without the field fall back to Chinese

## Usage

1. After launch an off-white ball appears at the bottom right of the screen
2. **To send a file link**: right-click → "Copy documents" → paste the direct URL and press Enter → switch to the chat window and press Ctrl+V
3. **To convert / compress a document**: drop the file onto the ball → pick "Convert" or "Compress" → pick a format or level → Start → when it finishes click "Open temp folder"
4. **To match photos**: right-click → "Face matching" → upload photos into both groups → Start → when it finishes click "Open temp folder" for the Word file
5. **To remove a watermark**: right-click → "Remove watermark" → pick an image → paint or box the watermark → Start → when it finishes click "Open temp folder" for the PNG

## Requirements

- Windows 10/11 (system WebView2)
- [Rust](https://rustup.rs/) 1.90+ (GNU toolchain) + MinGW (w64devkit with gcc/dlltool)
- Node.js 18+ (for the tauri CLI)

## Development and packaging

```bash
# install the tauri CLI
npm install

# dev mode (hot reload)
npm run dev

# build the Windows installer
npm run build                # same as: tauri build
npm run build:win            # same as: tauri build --bundles nsis
```

Output: `src-tauri\target\release\bundle\nsis\desktop-pet-tool_1.0.0_x64-setup.exe` (about 10.4 MB, NSIS wizard installer with a selectable install directory)

Offline build:

```powershell
cd src-tauri; $env:CARGO_NET_OFFLINE='true'; cargo build --offline --release
cd ..; $env:CARGO_NET_OFFLINE='true'; npx tauri build --bundles nsis
```

> When only the front end changed, touch `src-tauri\src\lib.rs` before packaging, otherwise the incremental build does not re-embed the `ui/` assets.
> If the terminal cannot find `cargo`, reopen it or run:
> ```powershell
> $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path;C:\w64devkit\w64devkit\bin"
> ```

## Testing

`tools\docs-smoke` is an end-to-end smoke test that reuses the real sources through `include!` and needs no Tauri runtime:

```powershell
cd tools\docs-smoke
$env:CARGO_NET_OFFLINE='true'
cargo run --offline --quiet                 # all assertions
cargo run --offline --quiet -- compress <file> small   # compress a single file directly
```

It covers images / Office / PDF / structured conversion / legacy `.doc` text extraction / table rendering into PDF; it prints `ALL PASS` when everything passes.

## Project layout

| Path | Purpose |
|---|---|
| `ui\index.html` | Floating-ball UI (paste bubble, document bubble, emotion-ball container, progress ring) |
| `ui\styles.css` | Styles (bubbles, document panel, chips, ring) |
| `ui\renderer.js` | Interaction logic (clicks/drag/expressions/downloads/document bubble state machine) |
| `ui\i18n.js` | Chinese/English dictionary, `t()` with placeholders, `data-i18n` static text application |
| `ui\settings.html` | Settings window (540 px wide) |
| `ui\docs.html` `ui\docs.js` `ui\docs.css` | The old standalone "document tool window" pages, **no longer opened** (logic moved into `renderer.js`), kept for later cleanup |
| `ui\vendor\emotion-ball\` | emotion-ball engine (rings / emotions / ball / engine, vendored as is) |
| `src-tauri\src\lib.rs` | Rust main process: windows and visible-region clipping, global mouse polling, dragging, downloads, clipboard, tray and context menu, config, document commands |
| `src-tauri\src\i18n.rs` | Main-process language state (`t(zh, en)`) for menus, tray, dialogs and document prompts |
| `src-tauri\src\drop_target.rs` | OLE drop target (dropped files → queue → notify the ball) |
| `src-tauri\src\docs\mod.rs` | Document job scheduling: format matrix, job thread, progress/done events, output folder |
| `src-tauri\src\face\models.rs` | Face model delivery: manifest verification, download, unpack into `face\v1`, `marker.json` |
| `src-tauri\src\face\mod.rs` | Face-matching job: group validation, sidecar thread (`catch_unwind`), `face-progress` events, Word output |
| `src-tauri\src\watermark\models.rs` | Watermark model delivery: manifest verification, download, unpack into `watermark\v1`; ort runtime reused from face |
| `src-tauri\src\watermark\mod.rs` | Watermark job: image validation, sidecar thread, `wm-progress` events, PNG output |
| `src-tauri\binaries\pdfconv\scripts\face_match.py` | Matching sidecar: detect / align / embed / assign / write Word, printing one JSON progress line per step |
| `src-tauri\binaries\pdfconv\scripts\watermark_erase.py` | Inpainting sidecar: mask redraw / dilation + feathering, LaMa inference, alpha-blended write-back, one JSON progress line per step |
| `tools\face-models\` | Model packaging script, manifest and upload example (for COS delivery) |
| `deploy\watermark-models\` | Watermark model manifest and upload notes (for COS delivery) |
| `src-tauri\src\docs\` other modules | `office_ops` / `image_ops` / `pdf_ops` / `pdf_render` / `pdf_text` / `pdf_font` / `pdf_stream` / `pdf_extract` / `pdf_write` / `legacy` / `from_pdf` / `office_write` / `to_pdf` / `text_ops` / `pptx_write` / `zip_min` / `doc_binary` |
| `src-tauri\tauri.conf.json` | Window and packaging configuration |
| `tools\docs-smoke\` | End-to-end smoke test and fixtures for the document features |
| `src-tauri\icons\` | App icons (tray and window) |

## Implementation notes

- **Pure Rust, no dependency on a local Office / WPS / LibreOffice / PDFium**: OOXML and CFB are unpacked and parsed in-house, PDF layout and rasterisation are in-house, and a Noto Sans SC subset is embedded
- **Legacy `.doc`**: reads `WordDocument` / `0Table` / `1Table` out of the CFB container, extracts text through `PlcPcd` pieces, handles field codes, table cells and paragraph control characters, then emits structured blocks
- **All document interaction stays on the ball**: the main window is 420×620, but `SetWindowRgn` keeps only the visible region at the bottom right, so the ball sits exactly where it used to; expanding a bubble only enlarges the clickable region
- **Event routing**: document progress / done events go to the main window (`docs-progress` / `docs-done`); there is no separate `docs` window any more
- **Drop safety**: a dropped file is only queued inside `IDropTarget::Drop`; the front end is notified after the OLE drag loop exits, so nothing drives WebView2 synchronously from the drop callback
- **Language switch**: the setting is saved through `config_set`, which rebuilds the tray menu and emits `config-changed` to the main and settings windows; the front end re-renders in place
- **One output folder**: `resolve_output_dir()` reads `temp_dir` from the config (default `%TEMP%\desktop-pet-tool`); "Open temp folder" and "Clean temp folder" act on every artefact

## Known limitations

- Legacy binary **`.xls` (BIFF8)** and **`.ppt`** are not wired in yet; such files report "no available target format"
- `.doc` cannot be compressed directly; `.doc → docx` tables become tab-separated paragraphs (`.doc → pdf` does produce a full table grid)
- `xlsx → pdf` exports only the first worksheet; `pptx → pdf` is a text-first layout ("text + images + page title"); `pdf → xlsx` carries text only and does not rebuild tables
- `docx` tables do not repeat the header row across pages
- PDF rasterisation does not parse CFF/Type1 outlines, and `ICCBased` is always treated as RGB
- Scanned PDFs or PDFs without ToUnicode produce no text and fail with an error; run OCR first
- The embedded font is Noto Sans SC only (sans-serif); no serif font is embedded
- Face matching only handles frontal or near-frontal faces: profiles, occlusion, backlight and sunglasses tend to be missed. The detection viewport is fixed at 640, so enlarging the whole image does not improve small-face recall — cropping the face does
- Face matching does no OCR, so printed names or ID numbers on the visa photo are ignored; when the two groups differ in size, the extra people are listed on the last page instead of being force-paired
- Watermark removal infills from the background around the selected area: large occlusions, strong texture changes, or a watermark over a busy pattern may leave faint traces along the patched edge; the CPU model takes tens of seconds on first load

## Config file

`%APPDATA%\com.desktop-pet-tool.float\config.json` — stores the temp folder path, always-on-top, autostart, close behaviour and language (`lang`).
