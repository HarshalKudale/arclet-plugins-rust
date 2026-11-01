# Browser Plugin

Shared browser controls for KnobDeck across Chrome, Edge, Brave, Firefox, and Zen, on Windows and
macOS. On macOS it finds browsers in `/Applications` / `~/Applications`, reads bookmarks and
history from `~/Library/Application Support`, and drives Chrome, Edge and Brave through their AppleScript interface (tabs, navigation, windows,
incognito), asking once for permission to control the browser. Firefox, Zen and page commands with no
scripting equivalent use the macOS shortcuts (⌘T, ⌘[, ⌃⌘F…).

## What it does

- Opens URLs, bookmarks, and history items.
- Triggers common browser shortcuts like refresh, private window, and zoom controls.
- Focuses the matching browser app before sending commands.
- Sends custom website-specific keybinds to the focused browser window.

## Good for

- Launching browser workflows from a wheel.
- Quick navigation commands during streaming or demos.
- Reusing one plugin across multiple browser families.
- Website shortcuts like YouTube playback controls while that site is open.
