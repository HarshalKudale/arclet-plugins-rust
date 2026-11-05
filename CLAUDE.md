# KnobDeck plugins (Rust) — working rules

Rust plugins for KnobDeck, built on `knobdeck-sdk` (`../knobdeck-sdk`). KnobDeck runs on **Windows,
macOS and Linux**; see README.md for the plugin list, build and packaging. SDK changes go to the
SDK repo through git (commit and push); plugins depend on its GitHub master, never a local path.

## Rule 1: keystrokes are the last resort

Applies to every plugin and to KnobDeck's built-in actions (`../knobdeck/CLAUDE.md` has the host copy).
Do **not** implement an action by sending a keyboard shortcut (`send_keys`, `ctx.send_keys`,
`keys_button`, posting CGEvents/SendInput) when a stable OS or app route exists. Shortcuts are
fragile:

- the app in focus may bind the same chord and swallow it (e.g. an app using Ctrl+→);
- users remap or disable shortcuts (System Settings → Keyboard Shortcuts, PowerToys…);
- keyboard layouts move keys; injected keys may lack flags real keyboards send (macOS Fn/NumericPad);
- timing and focus changes (the dial, the deck) can send the chord to the wrong window.

Prefer, in this order:

1. **An OS API.** Windows: Win32 / COM / WinRT through the `windows` crate (window management,
   `LockWorkStation`, audio endpoints, clipboard). macOS: public frameworks (Accessibility
   `AXUIElement`, CoreAudio, CoreGraphics, AppKit, Text Input Sources, IOKit). Linux: the
   compositor (KWin scripting, the KnobDeck GNOME Shell extension, via `linux-desktop`) and
   D-Bus services (KGlobalAccel, KRunner, Klipper, logind, NetworkManager, BlueZ,
   power-profiles-daemon, MPRIS, FileManager1).
2. **A long-stable private OS entry point**, resolved with `dlsym` / `GetProcAddress` at call time
   so its absence is a clear error and never a crash at launch (e.g. `CoreDockSendNotification`
   for Mission Control, `SACLockScreenImmediate` for Lock Screen).
3. **The target app's own interface**: its local API / socket (OBS websocket, Discord RPC, VLC HTTP,
   the VS Code / IntelliJ companions), its scripting dictionary (AppleScript `do script`,
   iTerm2 `write text`), or pressing its menu item through Accessibility (mac-tools pastes by
   pressing the front app's Edit ▸ Paste item, found by its ⌘V binding, not by sending ⌘V).
4. **Launching the system app / URL** (`open -b <bundle id>`, `ms-settings:` URIs).
5. **A keystroke**, only when none of the above exists. Then:
   - say why in a comment next to the action ("no API opens the Force Quit window; ⌥⌘Esc is
     reserved by loginwindow, so no app can capture it");
   - for a *system* shortcut, read the user's configured binding when the OS stores it (macOS
     `com.apple.symbolichotkeys`) and post exactly that, instead of assuming the default;
   - prefer system-reserved chords (handled by the OS before any app) over app-level ones;
   - use the `mod` token (⌘ on macOS, Ctrl on Windows) for cross-platform app shortcuts.

Keystrokes are fine when the action *is* typing (custom keybind actions the user configures, or
text the plugin must enter), and for in-app shortcuts sent to an app that
offers no other interface (browser tab shortcuts in Firefox/Zen).

When reviewing or adding an action, check it against this rule and mention any keystroke that
remains, with its reason.

Current decisions (keep this list up to date):

| Plugin | Route | Keystrokes left, and why |
|---|---|---|
| discord, obs, vlc, zoom, vscode, intellij, streamlabs, voicemod, elgato-lights | the app's / device's own API / pipe / websocket / HTTP | none |
| spotify | Windows SMTC, macOS AppleScript and Linux MPRIS for playback; Spotify Web API for the rest | none |
| twitch, youtube, weather | web APIs (Helix, YouTube Data, Open-Meteo) | none |
| clocks | none needed (time and timers only) | none |
| mac-tools | Dock (`CoreDockSendNotification`), `SACLockScreenImmediate`, Text Input Sources, Accessibility (windows, hide, Edit ▸ Paste), Screenshot app | Spaces ←/→ and Spotlight (no API; the user's configured system hotkey is posted), Force Quit ⌥⌘Esc (no API; reserved by loginwindow), ⌘V only for apps without a Paste menu item; Text Snippet types its text (text entry) |
| win-tools | Win32 (windows, audio sessions and endpoints, `LockWorkStation`), undocumented `IPolicyConfig` for the default audio device (no public API; unchanged since Windows 7), `ShellExecuteW` for Task View / Show Desktop / Run (`shell:::{CLSID}`), `ms-screenclip:`, `ms-settings:`, Explorer, Task Manager | virtual desktops (no public API), Emoji Win+. (no API), Ctrl+V for clipboard history and Text Snippet paste mode (no app-independent paste call); Text Snippet types its text (text entry) |
| linux-tools | KDE: KWin scripts over `org.kde.kwin.Scripting` (windows, desktops), KGlobalAccel `invokeShortcut` (KWin's / PowerDevil's own actions by name: Overview, Expose, Show Desktop, Turn Off Screen), KRunner, Klipper, Spectacle. GNOME: the KnobDeck Shell extension (`linux-desktop/gnome-extension`: windows, workspaces, overview, search, screenshot UI, clipboard), `org.gnome.ScreenSaver`. Both: logind, NetworkManager, BlueZ, power-profiles-daemon, FileManager1, `pactl` | Ctrl+V for clipboard history and Text Snippet (no app-independent paste request on Linux; Wayland lets no client act on another's text field) |
| browser-plugin | Linux KDE/GNOME: focusing the browser through the compositor (`linux-desktop`), URLs via the browser's program or `gio launch`. macOS Chromium (Chrome/Edge/Brave): AppleScript for tabs, navigation, windows, incognito, history/downloads/bookmarks pages; URLs via `open -a` / the browser exe | Firefox/Zen (no scripting interface), all browsers on Windows and Linux (no control interface without a debug flag), and page commands with no scripting equivalent (find, print, save, zoom, hard reload, reopen tab, full screen) |
| ai-stats | providers' usage endpoints, local transcripts | none |

## Rule 2: every OS stays working

- Cross-platform plugins ship one package per OS: ids `<name>-windows` / `<name>-macos` / `<name>-linux`
  (`format!("<name>-{}", std::env::consts::OS)`), `.platforms(&[std::env::consts::OS])` (actions
  inherit it). CI builds each on its own OS; `scripts/plugins.mjs` lists which plugins ship where.
- **Linux scope:** Debian/Ubuntu and Arch based distributions (not Fedora yet); KDE Plasma 6 and
  GNOME 45+, Wayland and X11. Window and desktop work goes through `linux-desktop` (one command set
  implemented in `kwin.js` and the GNOME extension; keep them in step and bump the extension's
  `version` in `metadata.json` when it changes). Other desktops get a clear "needs GNOME or KDE
  Plasma" error, never a silent no-op.
- **An app's plugin ships for Linux only when the app itself has an official Linux build** (from
  its vendor, or a distro package of that build), not a third-party client or workaround. Then
  check that the interface the plugin uses exists on Linux too. Not shipped: Streamlabs Desktop
  and Voicemod (no Linux build), Zoom (its local meeting API isn't documented for Linux).
- To ship a plugin, bump `version` in its `Cargo.toml` and in `index.json` (CI fails if they disagree) and merge to master; CI builds only plugins whose `index.json` version differs from the previous release, copies the rest over from it and publishes one release holding every plugin (see `.github/workflows/release.yml`).

- A plugin that runs on both OSes keeps one action set; OS differences live behind one `cfg` seam
  per concern (e.g. `win-tools`-style `win.rs` / `mac.rs` behind `mod os`).
- An OS-only plugin or action declares `.platforms(&["windows"])` / `.platforms(&["macos"])` / `.platforms(&["linux"])`.
  `win-tools` is Windows-only; `mac-tools` and `linux-tools` are its macOS and Linux counterparts.
- Don't regress one OS while working on another. Before finishing, run
  `./scripts/check-windows-from-mac.sh` (Windows type-check, no warnings; from Linux also
  `TARGET=aarch64-apple-darwin` for macOS) and `cargo test --workspace`.
- `fg_process` is the exe name on Windows, the bundle id on macOS and the window class / app id on Linux.

## Rule 3: verify, don't assume

- Drive a built plugin over the real stdio protocol (`hello` → `sync` → `request`), not just unit
  tests. Owning FFI wrappers must never be built around NULL (use `.then(|| ..)`, not
  `.then_some(..)`, for anything with a `Drop` that releases).
- Live tests that touch the user's screen (moving windows, opening Mission Control) are
  `#[ignore]` and act only on windows they create themselves, matched exactly (a terminal's title
  can contain the test command). Ask before running anything that takes over the user's display.
  On Linux, resize/maximize results need an unlocked screen: hidden clients don't acknowledge
  configures.

## Rule 3: a change to a plugin bumps its version

- Any code change to a plugin bumps its `version` in `Cargo.toml`.
- Put the same version in `index.json`. CI builds only plugins whose `index.json` version differs from the previous release's `registry.json` and copies every other plugin's artifact over from that release, so without the bump your change is not published.
- Icons are not release assets: the registry links `<dir>/icons/<icon>` by raw GitHub URL, so changing an icon file needs no version bump.
