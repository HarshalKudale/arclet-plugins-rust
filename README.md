# KnobDeck Plugins (Rust)

Rust ports of the first-party [KnobDeck](https://github.com/HarshalKudale/knobdeck) plugins in
[`../knobdeck-plugins`](../knobdeck-plugins), built on [`knobdeck-sdk`](../knobdeck-sdk-rust). Each plugin
generates the **same `actions.json` as its JS original** (same action ids, inputs, states), so
placed slices carry over.

Every cross-platform plugin ships as one package per OS (`<id>-windows`, `<id>-macos`, `<id>-linux`), each built on its own OS and declaring its `platforms`. `win-tools`, `mac-tools` and `linux-tools` are OS-specific. KnobDeck only offers and installs packages for its own OS.

**Linux** (Debian/Ubuntu and Arch based; KDE Plasma 6 and GNOME 45+, Wayland and X11): an app's
plugin ships for Linux only when the app has an official Linux build. Streamlabs Desktop and
Voicemod have none; Zoom's client does, but the local meeting API the plugin uses isn't documented
for it, so Zoom stays Windows/macOS for now. `linux-desktop` is a shared library crate (not a
plugin): the KWin / GNOME Shell window bridge, D-Bus helpers and desktop entries.

| Plugin | Description | OS |
|--------|-------------|----|
| [discord](./discord) | Discord voice controls over the desktop client's local RPC (named pipe on Windows, unix socket on macOS and Linux) | Windows, macOS, Linux |
| [browser-plugin](./browser-plugin) | Chrome / Edge / Brave / Firefox / Zen shortcuts, wheels, bookmarks, history (per-OS install paths, profiles and shortcuts) | Windows, macOS, Linux |
| [intellij](./intellij) | JetBrains IDE actions via the KnobDeck IntelliJ companion plugin | Windows, macOS, Linux |
| [obs](./obs) | OBS Studio control via obs-websocket v5 (own client, reconnecting) | Windows, macOS, Linux |
| [vscode](./vscode) | VS Code controls via the KnobDeck VS Code companion extension | Windows, macOS, Linux |
| [vlc](./vlc) | VLC media controls via its HTTP interface | Windows, macOS, Linux |
| [zoom](./zoom) | Zoom meeting controls via the local API | Windows, macOS |
| [win-tools](./win-tools) | Window management, per-app volume, clipboard history (`windows` crate) | Windows |
| [mac-tools](./mac-tools) | Window management (Accessibility API), window switcher, Spaces/system shortcuts, stats, audio devices + mic mute, clipboard history | macOS |
| [linux-tools](./linux-tools) | Window management (KWin scripting / KnobDeck GNOME extension), workspaces, desktop actions, stats, per-app volume, audio devices, power profiles, Wi-Fi/Bluetooth, clipboard history | Linux (KDE, GNOME) |
| [ai-stats](./ai-stats) | Claude and Codex plan limits (5 h / weekly gauges) and 30-day token usage, from the CLIs' local logins and transcripts | Windows, macOS, Linux |
| [elgato-lights](./elgato-lights) | Key Light, Ring Light and Light Strip over their local HTTP API, found by mDNS | Windows, macOS, Linux |
| [weather](./weather) | Current weather, 7-day and hourly forecast, air quality (Open-Meteo, no key) | Windows, macOS, Linux |
| [clocks](./clocks) | Clock for any time zone, world clock wheel, countdown timer, stopwatch | Windows, macOS, Linux |
| [streamlabs](./streamlabs) | Streamlabs Desktop over its local API (go live, record, replay, scenes, audio) | Windows, macOS |
| [voicemod](./voicemod) | Voicemod Control API: voices, voice changer, hear myself, soundboard | Windows, macOS |
| [spotify](./spotify) | Playback through the OS (SMTC / AppleScript / MPRIS); likes, shuffle, repeat, volume, playlists through the Web API | Windows, macOS, Linux |
| [twitch](./twitch) | Helix API: chat, chat modes, ads, stream info, markers, clips, viewers, channel monitor | Windows, macOS, Linux |
| [youtube](./youtube) | YouTube Data API: viewers, chat, ads, dashboard, go live / end stream | Windows, macOS, Linux |

`vscode-companion` and `intellij-companion` are not ported: they are an editor extension and a
Kotlin IDE plugin that run *inside* VS Code / IntelliJ, not KnobDeck SDK plugins. The Rust `vscode`
and `intellij` plugins talk to the originals unchanged.

Spotify, Twitch, Voicemod and YouTube take app credentials at build time from repository variables
(`SPOTIFY_CLIENT_ID`, `TWITCH_CLIENT_ID`, `VOICEMOD_CLIENT_KEY`, `GOOGLE_CLIENT_ID`,
`KNOBDECK_GOOGLE_TOKEN_URL`); without them the plugin's setup asks for the user's own. No client
secret is built into any plugin.

## Build

```bash
cargo install --path ../knobdeck-sdk-rust --bin knobdeck-plugin   # once

cd vlc
knobdeck-plugin dev        # debug build -> dist/vlc/ ("Load Unpacked" in KnobDeck)
knobdeck-plugin build      # release build -> self-contained dist/vlc/
knobdeck-plugin package    # dist/vlc.knobdeck
```

All crates share one Cargo workspace (`cargo build --workspace`, one `target/`).

The workspace uses the SDK's `macos-support` branch (the plugins need its `platforms` and per-OS
`executables` support). Drop `branch` in `Cargo.toml` once it is merged into the SDK's master.

### Checking Windows from a Mac (or Linux)

```bash
rustup target add x86_64-pc-windows-msvc   # once
./scripts/check-windows-from-mac.sh        # type-checks every plugin for Windows (no linking)
TARGET=aarch64-apple-darwin ./scripts/check-windows-from-mac.sh   # macOS, from Linux
```

### Building on Linux

`knobdeck-plugin package` writes the zip with `bsdtar` (Debian/Ubuntu: `libarchive-tools`, Arch:
`libarchive`). CI builds the Linux packages on Ubuntu 22.04, so they run on Debian 12+, Ubuntu
22.04+ and Arch.

## Differences from the JS plugins

- **No bundling / no Node runtime.** Each plugin ships as a native executable (`.exe` on Windows; plus a `<id>-setup` binary for plugins with setup); the manifest has no `runtime.language`.
- **Config and credentials.** Setup writes `.env` (and Discord its token) into the plugin data folder (`%APPDATA%/KnobDeck/plugin-data/<id>/` on Windows, `~/Library/Application Support/KnobDeck/plugin-data/<id>/` on macOS, the host's config folder on Linux, via `ctx.data_dir()`), which survives updates and reinstalls; the plugin reads it at start-up (`load_env()`, which also moves a `.env` left in the plugin folder by an older version). On an auth failure a plugin calls `mark_setup_required(reason)` and KnobDeck asks the user to run setup again. Discord never ships its client secret: `discord-setup` sends the authorization code to the KnobDeck website (`KNOBDECK_DISCORD_TOKEN_URL`, baked in from `discord/.env` by `build.rs` or set at runtime), which performs the token exchange; the plugin renews expired tokens through the same endpoint.
- **OBS** subscribes to the non-high-volume event set (no input-volume-meter spam, which the plugin never used).
- **Discord**: `auth.js` / `oauth.js` / `test/` in the JS plugin are not used by `plugin.js` and were not ported; the setup flow (native AUTHORIZE popup → token exchange) lives in `discord-setup`.
- **Browser plugin** reads bookmarks/history from a *copy* of the browser's SQLite file (bundled SQLite via `rusqlite`), because running browsers lock the original.
- **Error text.** Where the JS reported a message twice (`ctx.setError` then `throw`), Rust reports once.

## Verification

- `actions.json` generated by each Rust plugin is compared against the JS SDK's output for the same plugin (identical for all 8; the JS SDK's validator rejects the browser plugin's `combobox` input type, which the host supports — the Rust SDK allows it).
- Automated: SDK protocol test against a mock host, OBS client against a mock obs-websocket server (auth hash cross-checked with `obs-websocket-js`), Discord IPC handshake against a live client (`cargo test -p discord -- --ignored`), Win32/WASAPI/icon smoke tests.
- **Not exercised end-to-end** against the real services: VLC, Zoom, VS Code/IntelliJ companions, a real OBS instance, an authenticated Discord session, browser bookmarks/history reads, clipboard history, and the host app loading the generated `dist/`.
