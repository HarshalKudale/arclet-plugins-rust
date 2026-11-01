# Mac Tools

Window management and system tools for KnobDeck on **macOS** — the macOS counterpart of Win Tools.

## What it does

- **Current window:** minimize, fill the screen, restore, close, bring to front, hide the app,
  Fill / Restore and native Full Screen toggles (their state follows the focused window).
- **Position:** halves, quarters, thirds, center, fill a display, custom position/size, and move
  to the next display. Every action can target the current display or a chosen one.
- **Dynamic wheels:** pick any open window (with its app icon) and apply one of the actions above.
  **Switch to Window** brings the chosen window forward, including minimized ones.
- **Spaces and system:** previous/next Space, Mission Control, App Exposé, Show Desktop, Spotlight,
  Screenshot, Emoji & Symbols, Hide Others, Force Quit, Lock Screen, Sleep Display, and Finder /
  System Settings / Activity Monitor.
- **System Stats:** live CPU, GPU or memory use on the tile; pressing it opens Activity Monitor.
- **Audio:** Mute Microphone toggle, and wheels to switch the output or input device.
- **Text Snippet:** types saved text into whatever has focus, or with Paste mode on, copies it to
  the clipboard and pastes it.
- **Clipboard History:** the last 16 copies (text, rich text, images, files); picking one pastes it.

## Permissions

- **Accessibility** (already granted to KnobDeck) — moving, resizing and reading windows.
- Shortcut actions are sent by KnobDeck itself, like its built-in key actions.

## Differences from Win Tools

macOS doesn't let one app change another app's window level, opacity, frame style or mouse
handling, or set per-app volume without installing an audio driver. So **Always on Top, Opacity,
Transparency, Click-Through, Borderless, Send to Back and per-app volume are not included**,
rather than shipped as actions that silently do nothing.

- "Maximize" fills the display's visible area (the menu bar and Dock stay visible). Native full
  screen (its own Space) is the separate **Full Screen** toggle. Windows that size in steps, such as
  Terminal, may stop a row short of the bottom.
- macOS has no system clipboard history, so Mac Tools records copies **while it runs**, in memory
  only. Nothing is written to disk. Copies that apps mark as private (password managers use the
  `org.nspasteboard.ConcealedType` / `TransientType` markers) are never recorded.
- System actions call macOS directly instead of pressing shortcuts, so the app in front can't
  intercept them and remapped shortcuts don't break them: Mission Control, App Exposé and Show
  Desktop ask the Dock, Lock Screen uses the system lock call, Emoji & Symbols opens the Character
  Viewer, Hide Others and Paste go through Accessibility (Paste presses the app's own Edit ▸ Paste),
  and Screenshot opens the Screenshot app.
- macOS has no API to switch Spaces or open Spotlight, so those post **your** configured system
  shortcut (read from Keyboard Shortcuts; macOS handles it before any app sees it) and report it if
  you turned it off. Force Quit… sends ⌥⌘Esc, which macOS reserves for itself.
- GPU use is read from the graphics driver's statistics. Machines without a GPU driver (VMs) show "n/a".
