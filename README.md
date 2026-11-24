# <sub><img src="https://cdn.zspfx.com/refcanvas/icon.svg" width="32" height="32" alt=""></sub> refcanvas

A GPU-accelerated infinite canvas for digital artists, written in Rust + Bevy 0.20 with Vulkan rendering.

![Picture](https://cdn.zspfx.com/refcanvas/app_0.1.2-3.webp)

> Heads-up, LLM were used to built this; it is marely as a tool to translate my technical decisions into a code, I took full control over the design and software artitecture to ensure best possible quality and smooth experience using my software engineering knowledge.

## Design Goals

- Butterly smooth performance on canvas with thousands of images
- Gizmos and interactions that felt just right
- Free up VRAM whenever possible (More memory for DCC)
- Local-first design without relying on the Cloud

## This fork

Personal build for collecting references and Midjourney concept art. Projects stay in the same folder format (append-only JSONL journal, checkpoints, content-hashed images, 512px previews). Older projects still open: the new fields are optional.

### Doodle

- **B** brush, **E** eraser, **V** (or Escape) back to select. The same three buttons sit in the bar at the bottom of the window.
- One smooth brush. Width is uniform along the stroke (no pressure). **[** and **]** change it, in screen pixels at the moment you draw.
- Sixteen VGA colors in that bar. Strokes are stored as simplified point lists (Chaikin, then Ramer–Douglas–Peucker) and drawn as meshes.
- A stroke that **starts on an image** sticks to that image: it moves, scales, rotates, flips and crops with it, and is deleted with it. A stroke on empty canvas is its own node (select it and press Delete, or use the eraser).
- Right-click an image → **Hide strokes** / **Show strokes** (saved with the project, visible by default).
- **Hide strokes** / **Show strokes** in the bottom-right corner hides every stroke. That choice is remembered on this machine.

### Labels

- **T**, or right-click empty canvas → **Add label**. A short single- or multi-line label in IBM Plex Sans (the same 16 colors).
- Double-click to edit. Enter inserts a line; **Ctrl+Enter** or clicking away commits; Escape cancels.
- Move, scale, rotate and delete it like any other node. Scaling is how you change the size.
- With one label selected, a color in the bottom bar (or the right-click swatches) recolors it.

### Web images

- Paste an `http`/`https` URL with **Ctrl+V** (one URL per line is fine). The file is downloaded in the background into the project's `images/` store and placed as a node. The address is stored on the node.
- Dropping an image file still works. Dropping a URL (`text/uri-list`, or the URL text some browsers send) downloads it the same way.
- A failed download shows an orange line in the status bar. The frame is not blocked.
- Right-click an image that has a source address → **Copy source URL**.

### Links

- Right-click an image, frame or label → **Set link…**. `https://…` and app schemes such as `notion://…` are both fine.
- **Open link** in that menu opens it with the OS handler. Double-clicking an image or a frame that has a link does the same (double-clicking a label still edits the words).
- A small dot in the top-right corner of a node means it has a link.

### Building on Windows

From a checkout, with the MSVC toolchain and a Vulkan driver (the RTX 5070 Ti's current driver is enough):

```
cargo build --release
target\release\refcanvas.exe
```

Rust 1.85 or newer (edition 2024). This tree builds on Linux with the same command (`target/release/refcanvas`). A Windows `.exe` is not produced by the Linux build.

Browser drag-and-drop and `notion://` links need a check on the Windows machine; see the notes in the pull request. GIF animation, video and rich text are still out of scope.

## Features

- Drag & drop images
- Portable project folder / Open multiple projects as tabs
- Copy image to clipboard and paste from clipboard
- Crisp image pixels when deep zoom
- Gizmos control for single/multi-selection transformation
- Move/Scale/Crop smart snapping on visible images
- Arrow key focused image navigations
- Focus on selected images [F]
- Frame group of images [CTRL+G]
- Industry standard hotkeys
- Autosave, Undo and Redo [Z, CTRL+Y]
- Hold thousands of images in one canvas (5,000+ OK!)
- Touchscreen support (Surface devices)
- Support shared folder, multiple artists viewing/editing at the same time
- Always-on-top pin for keeping references above other apps

## Technical Features

> Benchmarking can be done by clicking on bottom right FPS label to uncap the 120FPS limit.

- First-class Linux support, also runs on Windows
- Vulkan backend renderer performance, benchmarked result with (2K~4K images scattered in clusters)
  - 1000~ FPS, 100 images
  - 900~ FPS, 200 images
  - 800~ FPS, 500 images
  - 200~ FPS, 5,000 images
- Automatic VRAM eviction
- Arrow key image navigations with ray casting, distance, and viewport boundary
- 0% CPU/GPU utilization while idle/unfocused
- Previews (512px) LOD generation

## Not Yet Supported / Planned

- OpenColorIO
- Rich text (plain labels are supported; see above)
- Animated GIF
- Videos
- Compare A/B test slider
- Arrangement compaction
- Change frame color

## Download & Installation

> When there's a new version the app will let you know.

### Linux

For Arch Linux you can download the tar file from [releases](https://github.com/zspfx/refcanvas/releases) then install with `sudo pacman -U ./refcanvas...x86_64.pkg.tar.zst`

For other Linux distros you can find the AppImage in [releases](https://github.com/zspfx/refcanvas/releases) page.

### Windows

For Windows installation see the [releases](https://github.com/zspfx/refcanvas/releases) page for pre-built executable.

### MacOS (Apple Silicon)

> Untested, please open an issue if you found a bug.

Dawin-only artifacts will be compiled through GitHub Actions once a new release is published, you may find them here in [releases](https://github.com/zspfx/refcanvas/releases) page.

Unzip the file first then move `RefCanvas.app` into `/Applications`. After that, remove the quarantine flag using `xattr -cr /Applications/RefCanvas.app`.

## License

Copyright (C) 2026 ZSPFX. RefCanvas is free software under **GPL-3.0-or-later**;
see [LICENSE](LICENSE) for the full text.
