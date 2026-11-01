# Win Tools

Advanced Windows window-management and system-volume tools for KnobDeck. Windows only; on macOS
use [Mac Tools](../mac-tools).

## What it does

- Snap, move, center, resize, maximize, restore, and close windows.
- Toggle always-on-top, borderless mode, transparency, and click-through behavior.
- Control app-specific volume, mute, and monitor movement.
- Includes dynamic wheels for choosing target windows live.
- Switch, create and close virtual desktops; Task View, Show Desktop, Lock PC, Screen Snip,
  File Explorer, Settings, Run, Emoji Panel and Task Manager shortcuts.
- Text Snippet: types saved text into the focused window, or with Paste mode on, copies it to the
  clipboard and pastes it.
- System Stats button: shows live CPU, GPU or memory use on the tile and opens Task Manager when pressed.
- Position and monitor moves now apply real window bounds, and moved windows are
  raised on the target monitor without taking focus away from the current app.

## Good for

- Stream decks and productivity wheels.
- Multi-monitor workflows.
- Fast window layout changes while gaming or working.

Task View, Show Desktop, Run, Screen Snip, Settings, File Explorer and Task Manager open through the
Windows shell (`ShellExecute`) rather than shortcuts, so the focused app can't intercept them.
Virtual desktop switching and the Emoji Panel still use their Win-key shortcuts: Windows has no
public API for them.
