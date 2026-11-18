# dpt

English | [中文](README.md)

A **Tauri** desktop floating ball.
The installer is about **22 MB** (it uses the system WebView2, with no bundled browser engine; most of the size comes from the embedded Chinese font subset).

## Features

### 1. The floating ball

- **Off-white round ball**: drag it anywhere on the desktop, always on top, expressions rendered live by the emotion-ball engine (SVG + plain JS, zero dependencies)
- **App icon**: a dark graphite rounded square, a cream floating ball and a pale lilac cleanup orbit, with no text. The same artwork ships as a multi-size ICO (16/24/32/48/64/128/256), a 512 px PNG and a 32 px tray image; the small sizes use slightly heavier facial features so the face remains readable at 16 px
- **Left click**: wake up / dismiss the bubble that is currently open. **Left click no longer opens the input box**; the paste bubble is opened only from the right-click menu item "Copy documents"
- **Right-click menu** (two sections):
  - Tools: **Copy documents** / **Document tools** / **File search** / **Face matching** / **Remove watermark** / **System tools** (submenu: Junk cleanup / C: drive cleanup / Large files)
  - App: **Open temp folder** / **Clean temp folder** / **Launch at login** / **Settings** / **Quit**
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

All outputs land in the **temp folder** (default `%TEMP%\desktop-pet-tool`); "Open temp folder" opens it with **this run's files selected**, the same folder as the files downloaded by "Copy documents", so "Clean temp folder" wipes everything at once.

### 4. File search

Right-click "File search" → a bubble appears next to the ball. It matches file names fuzzily and lists the relevant files on this PC or on a network share within seconds:

1. **Source folder (optional)**: leave empty to search every local disk (fixed drives + USB). Pick a folder with **Browse**, or type a path, to search only that folder — local folders, mapped drives and `\\server\share` all work; entering just a server name such as `\\DS423` enumerates every disk share on that server and searches them all
2. **Keyword**: part of the file name is enough, case-insensitive; separate words with spaces to require all of them
3. **Ranking**: prefix hit → contains → all keywords → in-order fuzzy (`年度报告` finds `2024年度总结报告`)
4. **Result row**: file name, folder, size and modified date; click the row or "Reveal" to select it in Explorer, or "Open" to launch it with the default app
5. **Bounded**: at most 300 rows (scanning stops as soon as the cap is reached), system folders and junctions are skipped, and you can stop at any time; the summary shows how many entries were scanned and how long it took

Search is read-only: it never moves, renames or deletes anything.

### 5. Face matching

Right-click "Face matching" opens the same kind of bubble, with **two upload groups**. It pairs the people in the two groups one-to-one and produces a single Word file:

1. **Two upload areas**: `Group 1` / `Group 2`, each taking several photos (remove one by one, or add more); supported formats are `png / jpg / jpeg / bmp / tif / webp`, and non-images are rejected with a message
2. "Start" → the **progress ring around the ball plus a percentage pill**, while the bubble shows the current stage (preparing / downloading det_10g.onnx / Group 1 photo5.png / writing Word)
3. On completion a result bubble appears: how many pairs were found, how many were left unmatched, the Word path and an "Open temp folder" button

Matching rules:

- **One-to-one across the two groups**: each photo in group 1 pairs with at most one photo of group 2 and vice versa, so no photo is reused; pairing is a globally optimal assignment, not per-file maximum, and low-similarity combinations are left unpaired
- **Face only**: detect → five-point alignment → embedding → cosine similarity. **No OCR**, so printed names or ID numbers on the visa photo play no part
- **Local CPU inference**: InsightFace ONNX — SCRFD `det_10g` for detection, ResNet50 `w600k_r50` for embeddings; thread count is `min(4, CPU-1)`, and a few dozen photos usually take ten-odd seconds

Model delivery:

- Models and the onnxruntime dependencies are **downloaded on first use** into `<install dir>\data\face\v1\` (for example `D:\tools\dpt\data\face\v1\`; `models\` + `deps\` + `marker.json`, about 233 MB) and work offline afterwards
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

### 6. Remove watermark

Right-click "Remove watermark" opens the same kind of bubble. Pick an image, paint or box the watermark on the canvas, and a local model fills in the selected area, producing a new PNG:

1. **Pick an image**: supported formats are `png / jpg / jpeg / bmp / tif / webp`, non-images are rejected with a message. The canvas fits the image on first load; **scroll the wheel while the cursor is over the canvas to zoom in / out (0.4–4×)**, with the image layer and the mask layer scaled together
2. **Two ways to mark the area**: a **Brush** (adjustable size) or a **Box** rectangle; strokes can be undone or cleared, and their coordinates are mapped back to the original image by the current display scale
3. Click "Remove" → the **progress ring around the ball plus a percentage pill**, while the bubble shows the current stage (preparing model / inferring)
4. On completion a result bubble shows the output path plus an "Open temp folder" button; the output lands in the shared temp folder

Inpainting notes:

- **Local CPU inference**: a LaMa ONNX inpainting model (`inpainting_lama.onnx`, about 88 MB); the onnxruntime runtime is reused from face matching, so it is not downloaded twice
- **Models are downloaded on first use** into `<install dir>\data\watermark\v1\` (for example `D:\tools\dpt\data\watermark\v1\`; `models\` + `deps\` + `marker.json`) and work offline afterwards; deps reuse `data\face\v1\deps` first, and fall back to downloading the same ort package only when the face copy is absent
- Pipeline: strokes are redrawn into a mask → dilated twice plus Gaussian feathering → inferred at 512×512 → only the mask region is blended back with the feathered alpha, keeping the original image outside the mask so edges stay soft
- The manifest and sha256 values are documented in `deploy\watermark-models\README.md`; you can also place the onnx manually into `models\` for offline use

### 7. Disk cleanup

Right-click "System tools" → "Junk cleanup" / "C: drive cleanup" / "Large files" → a **1000×760 window** opens (resizable) with three tabs sharing one backend. Scans run off the main thread and can be cancelled at any time.

**Junk cleanup**

- **15 categories**: user temp files, Windows temp folder, Windows update cache, Delivery Optimization cache, thumbnails and icon cache, shader cache, crash dumps, Windows error reports, component (CBS) logs, system internet cache, browser cache (Chrome / Edge, off by default), Recycle Bin (off by default), old drivers, uninstall leftovers, registry leftovers
- Scanning is **read-only**: sizes and item counts are shown first, nothing is deleted until you tick a category and confirm; each row can be expanded to show paths and sizes
- Old drivers: enumerated with `pnputil /enum-drivers`, drivers referenced by `HKLM\SYSTEM\...\Class` are kept, and every package is exported with `pnputil /export-driver` into `%APPDATA%\com.desktop-pet-tool.float\cleanup-backup\drivers-<timestamp>\` before removal
- Uninstall leftovers: three Uninstall registry roots are compared against first-level folders under Program Files / ProgramData; entries without an uninstaller that still occupy space are listed and **moved to the Recycle Bin** (restorable)
- Registry leftovers: only uninstall entries whose icon and install path both point at missing paths and do not use `msiexec`; a `.reg` backup is exported first, then `RegDeleteTreeW` runs restricted to the `...\CurrentVersion\Uninstall\` subtree
- **Recycle Bin, leftovers, registry, old drivers always require a second confirmation** that lists sizes, counts or the affected paths

**Large files**

- Pick drives and a threshold (100 MB / 300 MB / 500 MB / 1 GB). A **bounded-concurrency** walker (2 threads by default, at most 8, scaled by CPU cores) runs without following junctions and skips `C:\Windows`, `$Recycle.Bin`, `System Volume Information`, `Recovery` and `Boot`
- `pagefile.sys` / `hiberfil.sys` / `swapfile.sys` / `memory.dmp` are protected and never appear in the list
- Results are sorted by size with path, size and modified time; rows can be ticked individually or all at once, and "Open location" reveals the file. Deletion only accepts **paths that came from the current scan** and re-validates each file
- The backend keeps up to 4000 rows; the UI renders the largest 500 to stay responsive

**C: drive cleanup**

- **Where the space went**: first-level folders are aggregated into a bar chart (`C:\Users\<user>` is expanded one level automatically); click a folder to drill down and go back up. Files directly under the C: root (`pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, etc.) are grouped into a read-only **Root system files** row and are not treated as a drillable folder
- **System cleanup**: hibernation file, page file, Windows.old, Recycle Bin, restore points, component store
  - Hibernation: `powercfg -h off` (also turns hibernation off)
  - Windows.old: `takeown` + `icacls` + `rd`
  - Restore points: `vssadmin delete shadows /for=C: /all /quiet`
  - Component store: opening the C: tab automatically runs `DISM /AnalyzeComponentStore` to read the actual usage (administrator rights, usually tens of seconds) and shows it on the card; cleanup uses `dism /online /cleanup-image /startcomponentcleanup`
  - The page file is reported but never deleted
- **App cache cleanup**: scans and lists every cache grouped by app, covering 20+ cache groups for WeChat, QQ, WeCom, Feishu, DingTalk, WPS, CapCut, Premiere, Photoshop, npm, conda, pip, JetBrains, Visual Studio, Codex, Claude, Doubao Work and WorkBuddy
  - Each cache item is split into "7 days ago / 30 days ago / 90 days ago / 6 months ago / earlier"; each bucket can be selected independently. The scan timestamp is fixed so cleanup never removes files created after the scan
  - Only the cache, log and temp folders listed in the rules are touched; chat databases, sessions and project folders are never included
  - Chat images / videos / received files from WeChat, QQ and WeCom, WPS auto backups and Premiere / Photoshop auto-saves are historical content: marked **risky**, off by default, and deleting them affects message history or recovery
  - A second confirmation is required. Only regular files in the selected age buckets and outside reparse points are deleted; `.db` / `.sqlite` / `.db-wal` database files are protected. Files locked by a running app are counted as failures
  - WeChat 4.x reads its custom data drive from `%APPDATA%\Tencent\xwechat\config\*.ini` (for example `D:\data`) and scans `xwechat_files` there; other apps follow their common default paths, so an app that is not installed or uses a custom data folder will not appear
- **App migration**: lists apps installed on C: (name / version / install path / folder size) and can move them to another fixed drive
  - `robocopy /E /MOVE` into `<drive>:\AppMoved\<folder>` → remove the empty source folder → `cmd /c mklink /J` leaves a **junction** at the original path, so the app still starts from where it was installed
  - Any failure moves the content back to the original path; the app is never left half-migrated
  - Pre-flight checks: target is a fixed drive, is not C:, and has at least folder size + 512 MB free
- System cleanup and migration need administrator rights; the window detects a standard user and offers **Restart as admin**

Progress is pushed over the `cleanup-progress` event (`{task, files, bytes}` or `{task, stage}`, throttled to 180 ms).

### 8. File shred

Once installed, the current user's Explorer context menu gains a **Shred file** entry (written under HKCU, so no administrator rights are needed):

- Select any file or folder in Explorer → right-click → **Shred file**; the ball opens a confirmation bubble listing names, sizes and file counts (on Windows 11 use "Show more options" in the collapsed context menu first)
- "End processes holding the files" is checked by default; nothing is deleted until you press **Shred** — the context menu itself never deletes anything
- Every file is overwritten end to end with random data (1 MiB chunks, xorshift64), deleted, then folders are removed deepest-first. Read-only / hidden / system attributes are cleared first
- When a delete fails, Restart Manager names the processes holding the file; after confirmation they are terminated (system-critical ones such as `explorer`, `lsass`, `svchost` and this app are skipped), the app waits for handles to be released and retries. If it still fails and the process is elevated, the path is registered with `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` for deletion on the next restart
- Drive roots and `C:\Windows` itself are refused; symbolic links / junctions are unlinked without touching the target; at most 200,000 entries are expanded per run
- Multi-selection makes Explorer start one process per item; a named mutex plus a named pipe merges those requests into the running ball instead of spawning several balls
- Progress is throttled to 120 ms and can be cancelled at any time; the finish page reports freed bytes, terminated processes and items that could not be deleted
- Menu text follows the UI language (`文件粉碎` / `Shred file` / `Shred folder`), the registry is rewritten and Explorer refreshed on language change, and the NSIS hook removes the keys on uninstall

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
- The whole main-process side follows: right-click menu, tray menu, system dialog titles, document job prompts, the Explorer **Shred file** label
- The front end follows too: paste bubble, document bubble, file-search bubble, face-matching bubble, watermark bubble, shred bubble, result rows, settings page
- The choice is stored in `config.json` as `lang` (`zh` | `en`); old config files without the field fall back to Chinese

## Usage

1. After launch an off-white ball appears at the bottom right of the screen
2. **To send a file link**: right-click → "Copy documents" → paste the direct URL and press Enter → switch to the chat window and press Ctrl+V
3. **To convert / compress a document**: drop the file onto the ball → pick "Convert" or "Compress" → pick a format or level → Start → when it finishes click "Open temp folder"
4. **To find a file**: right-click → "File search" → type part of the name → leave the source folder empty for this PC, or click "Browse" to pick a folder / share → Start → click "Reveal" on a row to select it in Explorer, or "Open" to launch it
5. **To match photos**: right-click → "Face matching" → upload photos into both groups → Start → when it finishes click "Open temp folder" for the Word file
6. **To remove a watermark**: right-click → "Remove watermark" → pick an image → paint or box the watermark → Start → when it finishes click "Open temp folder" for the PNG
7. **To free disk space**: right-click → "System tools" → "Junk cleanup" / "C: drive cleanup" / "Large files" → scan, tick, confirm; the C: tab also shows what occupies the drive, cleans app caches by age bucket, and can migrate apps to another disk

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

Output: `src-tauri\target\release\bundle\nsis\dpt_<version>_x64-setup.exe` (about 10.4 MB, NSIS wizard installer with a selectable install directory)

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
| `ui\cleanup.html` `ui\cleanup.css` `ui\cleanup.js` | Disk cleanup window: junk / large files / C: cleanup tabs |
| `src-tauri\src\cleanup\mod.rs` | Cleanup entry: task state, `cleanup-progress` events, disk/permission/path helpers, PowerShell `-EncodedCommand` runner, all cleanup commands |
| `src-tauri\src\cleanup\walk.rs` | Bounded-concurrency walker: size totals, large-file collection, folder aggregation, cancellable, modified-time/extension filters, no junction following |
| `src-tauri\src\cleanup\junk.rs` | 15 junk rules, scanning and cleaning (recycle bin, leftovers, drivers, registry) |
| `src-tauri\src\cleanup\reg.rs` | Registry reads, uninstall leftovers, orphan entries, old driver enumeration/backup/removal, apps on C: |
| `src-tauri\src\cleanup\large.rs` | Large-file scan and validated deletion |
| `src-tauri\src\cleanup\cdrive.rs` | C: space analysis, system cleanup actions, app sizes and migration |
| `src-tauri\src\cleanup\cache.rs` | App cache rules, per-app/age-bucket scan, risky chat media marking and guarded cleanup |
| `ui\docs.html` `ui\docs.js` `ui\docs.css` | The old standalone "document tool window" pages, **no longer opened** (logic moved into `renderer.js`), kept for later cleanup |
| `ui\vendor\emotion-ball\` | emotion-ball engine (rings / emotions / ball / engine, vendored as is) |
| `src-tauri\src\lib.rs` | Rust main process: windows and visible-region clipping, global mouse polling, dragging, downloads, clipboard, tray and context menu, config, document and disk-cleanup commands |
| `src-tauri\src\i18n.rs` | Main-process language state (`t(zh, en)`) for menus, tray, dialogs and document prompts |
| `src-tauri\src\drop_target.rs` | OLE drop target (dropped files → queue → notify the ball) |
| `src-tauri\src\docs\mod.rs` | Document job scheduling: format matrix, job thread, progress/done events, output folder |
| `src-tauri\src\face\models.rs` | Face model delivery: manifest verification, download, unpack into `face\v1`, `marker.json` |
| `src-tauri\src\face\mod.rs` | Face-matching job: group validation, sidecar thread (`catch_unwind`), `face-progress` events, Word output |
| `src-tauri\src\watermark\models.rs` | Watermark model delivery: manifest verification, download, unpack into `watermark\v1`; ort runtime reused from face |
| `src-tauri\src\watermark\mod.rs` | Watermark job: image validation, sidecar thread, `wm-progress` events, PNG output |
| `src-tauri\src\filesearch\mod.rs` | File-search job: drive / share enumeration, background scan threads, `fs-batch` / `fs-progress` / `fs-done` events, reveal and open |
| `src-tauri\src\filesearch\engine.rs` | File-search pure logic: keyword scoring, share-path normalization, skipped folders and depth-first walk (no tauri dependency, testable on its own) |
| `src-tauri\src\single.rs` | Single-instance mutex + named-pipe IPC: merges the processes Explorer starts for a multi-selection into one ball; `--fetch-pdf-deps` / `--elevated` are exempt |
| `src-tauri\src\shred\mod.rs` | Shred entry point: path queue, confirmation measurement, `shred-progress` / `shred-done` events, context-menu self-check |
| `src-tauri\src\shred\engine.rs` | Shred engine: walk and measure, random overwrite, delete with retry, delete-on-reboot, throttled progress (unit tests included) |
| `src-tauri\src\shred\shell.rs` | Registers / removes the Explorer verbs under `HKCU\Software\Classes\*\shell` and `Directory\shell`, then refreshes Explorer with `SHChangeNotify` |
| `src-tauri\src\shred\lock.rs` | Restart Manager lock detection and process termination (system-critical processes skipped) |
| `src-tauri\binaries\pdfconv\scripts\face_match.py` | Matching sidecar: detect / align / embed / assign / write Word, printing one JSON progress line per step |
| `src-tauri\binaries\pdfconv\scripts\watermark_erase.py` | Inpainting sidecar: mask redraw / dilation + feathering, LaMa inference, alpha-blended write-back, one JSON progress line per step |
| `tools\face-models\` | Model packaging script, manifest and upload example (for COS delivery) |
| `deploy\watermark-models\` | Watermark model manifest and upload notes (for COS delivery) |
| `src-tauri\src\docs\` other modules | `office_ops` / `image_ops` / `pdf_ops` / `pdf_render` / `pdf_text` / `pdf_font` / `pdf_stream` / `pdf_extract` / `pdf_write` / `legacy` / `from_pdf` / `office_write` / `to_pdf` / `text_ops` / `pptx_write` / `zip_min` / `doc_binary` |
| `src-tauri\tauri.conf.json` | Window and packaging configuration |
| `tools\docs-smoke\` | End-to-end smoke test and fixtures for the document features |
| `src-tauri\icons\` | App icons: multi-size `icon.ico` (16/24/32/48/64/128/256), `icon.png` (512 px) and tray `tray.png` (32 px) |

## Implementation notes

- **Pure Rust, no dependency on a local Office / WPS / LibreOffice / PDFium**: OOXML and CFB are unpacked and parsed in-house, PDF layout and rasterisation are in-house, and a Noto Sans SC subset is embedded
- **Legacy `.doc`**: reads `WordDocument` / `0Table` / `1Table` out of the CFB container, extracts text through `PlcPcd` pieces, handles field codes, table cells and paragraph control characters, then emits structured blocks
- **All document interaction stays on the ball**: the main window is 420×620, but `SetWindowRgn` keeps only the visible region at the bottom right, so the ball sits exactly where it used to; expanding a bubble only enlarges the clickable region
- **Event routing**: document progress / done events go to the main window (`docs-progress` / `docs-done`); there is no separate `docs` window any more
- **Drop safety**: a dropped file is only queued inside `IDropTarget::Drop`; the front end is notified after the OLE drag loop exits, so nothing drives WebView2 synchronously from the drop callback
- **Language switch**: the setting is saved through `config_set`, which rebuilds the tray menu and emits `config-changed` to the main and settings windows; the front end re-renders in place
- **One output folder**: `resolve_output_dir()` reads `temp_dir` from the config (default `%TEMP%\desktop-pet-tool`); "Open temp folder" opens it with the current run's outputs selected, and "Clean temp folder" acts on every artefact

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
- Old-driver enumeration depends on `pnputil`: a standard user only sees a "requires administrator" note
- Shredding overwrites in software: SSD TRIM / wear levelling, cloud-sync version history and filesystem leftovers cannot be fully erased by any app. Files that cannot be deleted at all (mostly those needing administrator rights) are only scheduled for deletion on the next restart and stay in place until then
- App migration can fail for apps with services, drivers or self-updaters, and some apps refuse to start after their path changes. Quit the app before migrating
- C: analysis walks every file and can take minutes on a full or mechanical drive; it is cancellable. Only first-level folders are aggregated (`Users\<user>` goes one level deeper), and you can drill down for more
- DISM component cleanup can take 5–30 minutes; keep the window open

## Config file

`%APPDATA%\com.desktop-pet-tool.float\config.json` — stores the temp folder path, always-on-top, autostart, close behaviour and language (`lang`). The folder keeps the identifier `com.desktop-pet-tool.float` and does not change with the `dpt` display name.
