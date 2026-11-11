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
- The front end follows too: paste bubble, document bubble, result rows, settings page
- The choice is stored in `config.json` as `lang` (`zh` | `en`); old config files without the field fall back to Chinese

## Usage

1. After launch an off-white ball appears at the bottom right of the screen
2. **To send a file link**: right-click → "Copy documents" → paste the direct URL and press Enter → switch to the chat window and press Ctrl+V
3. **To convert / compress a document**: drop the file onto the ball → pick "Convert" or "Compress" → pick a format or level → Start → when it finishes click "Open temp folder"

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

## Config file

`%APPDATA%\com.desktop-pet-tool.float\config.json` — stores the temp folder path, always-on-top, autostart, close behaviour and language (`lang`).
