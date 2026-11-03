# Stream Deck plugin analysis — top 20 and KnobDeck coverage

Snapshot: 2026-11-03. Purpose: pick which Stream Deck plugins to adapt, list each one's actions, how it
works, its Windows/macOS support, and what KnobDeck already has, so adaptations do not miss actions.

## Method and sources

- **Size of the catalogue:** the Elgato Marketplace lists **1,764 Stream Deck plugins** (`count` from the
  Marketplace API, 2026-11-03).
- **Ranking:** Marketplace API `https://mp-gateway.elgato.com/products?extension=plugins&ordering=-download_count`.
  The Marketplace does not show download counts, but this ordering works. Elgato's own end-of-2025 post
  calls Volume Controller "the most downloaded plugin", which matches #1 here.
- **OS support:** the `OS` tag and each version's `os_versions` (minimum OS) from
  `https://mp-gateway.elgato.com/products/<id>`.
- **Actions:** the Marketplace description, the plugin's open-source `manifest.json` (Fred Emmott's
  plugins) or the vendor docs (docs.barraider.com). Most plugin packages need a signed-in Marketplace
  account to download, so manifests of closed-source plugins were **not** read. Action lists marked
  *(partial)* come from marketing text and need checking against the real plugin before we build.

Legend: **W** = Windows, **M** = macOS. "Min" = minimum OS from the Marketplace.

---

## 1. Top 20 at a glance

| # | Plugin | Author | W | M | Min (W / M) | Dial | Needs app/account | KnobDeck today | Adapt as |
|---|--------|--------|---|---|-------------|------|-------------------|----------------|----------|
| 1 | Volume Controller | Elgato | ✅ | ✅ (devices only; per-app is W only) | 10 / 10.15 | ✅ | — | **Partial**: win-tools app volume, mac-tools devices | extend win-tools / mac-tools |
| 2 | OBS Studio | Elgato | ✅ | ✅ | 10 / 10.11 | ✅ | OBS (websocket) | **Covered**: `obs` | small additions |
| 3 | Discord | Elgato | ✅ | ✅ | 10 / 10.15 | ✅ | Discord desktop | **Covered, more than SD**: `discord` | — |
| 4 | Voicemod | Voicemod | ✅ | ✅ | 10 / 10.15 | — | Voicemod app | **Missing** | new `voicemod` |
| 5 | Spotify Integration | BarRaider | ✅ | ❌ | 10 / — | ✅ | Spotify Premium + dev app | **Missing** | new `spotify` (merge 5 + 8) |
| 6 | Control Center | Elgato | ✅ | ✅ | 10 / 10.13 | ✅ | Elgato lights on LAN | **Missing** | new `elgato-lights` |
| 7 | Twitch | Elgato | ✅ | ✅ | 10 / 10.11 | ❌ | Twitch account | **Missing** | new `twitch` (merge 7 + 17) |
| 8 | Spotify | Elgato | ✅ | ✅ | 10 / 12 | ✅ | Spotify desktop (+ sign-in for library) | **Missing** | new `spotify` |
| 9 | YouTube | Elgato | ✅ | ✅ | 10 / 10.11 | ❌ | Google account | **Missing** | new `youtube` |
| 10 | Audio Switcher | Fred Emmott | ✅ | ✅ | 10 / 10.11 | — | — | **Partial**: mac-tools only | extend win-tools |
| 11 | Win Tools | BarRaider | ✅ | ❌ | 10 / — | ✅ | — | **Partial**: `win-tools` | extend win-tools (+ mac-tools twins) |
| 12 | OBS Tools | BarRaider | ✅ | ❌ | 10 / — | ✅ | OBS (websocket) | **Partial**: `obs` | extend `obs` |
| 13 | Clocks | Elgato | ✅ | ✅ | 10 / 10.11 | — | — | **Missing** | new `clocks` (or host built-in) |
| 14 | Wave Link (Legacy) | Elgato | ✅ | ✅ | 10 / 10.14 | ✅ | Wave Link 2 + Elgato mic | **Missing** | defer (hardware-specific) |
| 15 | SuperMacro | BarRaider | ✅ | ❌ | 10 / — | ❌ | — | **Partial**: host keybind/multi/delay | extend host (macro action) |
| 16 | Audio Mute | Fred Emmott | ✅ | ✅ | 10 / 10.15 | — | — | **Partial**: mac-tools only | extend win-tools |
| 17 | Twitch Tools | BarRaider | ✅ | ❌ | 10 / — | ✅ | Twitch account | **Missing** | merge into `twitch` |
| 18 | Weather | Elgato | ✅ | ✅ | 10 / 10.15 | ✅ | — (Marketplace acct) | **Missing** | new `weather` |
| 19 | Streamlabs | Streamlabs | ✅ | ✅ | 10 / 10.11 | — | Streamlabs Desktop | **Missing** | new `streamlabs` |
| 20 | Windows Mover & Resizer | BarRaider | ✅ | ❌ | 10 / — | ✅ | — | **Mostly covered**: win-tools / mac-tools | small additions |

Counts: 14 of 20 run on both OSes (#1's per-app volume is Windows-only), 6 are Windows-only
(#5, #11, #12, #15, #17, #20), none are macOS-only. KnobDeck already covers 3 fully or mostly
(#2, #3, #20) and 6 partly (#1, #10, #11, #12, #15, #16). After merging duplicates
(Spotify ×2, Twitch ×2) the **new plugins** are: Voicemod, Spotify, Elgato Lights, Twitch, YouTube,
Clocks, Weather, Streamlabs (+ Wave Link later).

---

## 2. Existing KnobDeck plugins (baseline)

Source: each plugin's generated `dist/<id>/actions.json` (mac-tools from `src/main.rs`).
Type in brackets: button / toggle / hold / slider / wheel.

### Host built-in actions (`Knobdeck/app/src-tauri/src/native_compiler.rs`, `cmd_actions.rs`)
Keybind (presets + custom), Launch app (focus-if-open, args, cwd), Launch shortcut, Open path,
Open file explorer, OS Settings, Open terminal, Run command (hidden/visible), Delay, Lock, Sleep,
Shut down, Restart, Sign out, Empty recycle bin/Trash, Screenshot, Play/Pause, Next/Prev track,
Seek ±N s, Volume up/down/mute, Volume slider, Open URL. Structural kinds: Multi (multi-action),
Multi-toggle, Folder, Dynamic wheel, Link.

### discord (W, M) — Discord desktop local RPC (named pipe / unix socket)
Mute [toggle], Deafen [toggle], Push to Talk [hold], Push to Mute [hold], Voice mode toggle (VAD/PTT),
Join voice channel, Leave voice channel, Text channel, Server jump, Open DM, Video toggle,
Stream toggle, Volume control, Output volume [slider], User volume, Set audio device, Soundboard,
Voice-channel wheel, Call member mute wheel, Activity preset, Soundboard wheel, Notifications toggle.

### obs (W, M) — obs-websocket v5
Record, Record pause, Chapter marker, Stream, Virtual cam, Replay buffer, Save replay,
Scene collection, Profile, Scene, Source visibility, Audio mixer (mute), Media source control,
Studio mode, Transition, Filter, Screenshot, Scene transition (preview → program).

### vscode (W, M) — KnobDeck VS Code Companion extension (local HTTP)
Command palette, Quick open, Explorer, Search, Source control, Extensions, Terminal, Problems,
Save all, Format document, Toggle sidebar, Toggle panel, Zen mode, Split editor, New terminal,
Start/Stop debugging, Run task, Focus terminal, Open editor, Custom command, Task wheel,
Debug wheel, Open-editor wheel.

### intellij (W, M) — KnobDeck IntelliJ Companion IDE plugin (local HTTP)
Open action browser, Custom action, Update project, New project, Run, Debug, Step over/into/out,
Resume, Pause, Search everywhere, Stop, Project structure, Reset frame, Terminal, Claude Code.

### vlc (W, M) — VLC HTTP interface
Playback status, Play/Pause, Stop, Next, Previous, Fullscreen, Seek ±10 s, Volume ±, Open URL.

### zoom (W, M) — Zoom local API
Meeting status, Mute, Push to talk, Video, Screen share, Raise hand, Reaction, Recording,
Leave, End (host), Participants panel, Chat panel.

### browser-plugin (W, M) — AppleScript on macOS Chromium; keystrokes elsewhere
New tab/window/private window, Close tab, Reopen tab, Next/Prev tab, Reload, Hard reload, Back,
Forward, Address bar, Downloads, History, Bookmarks manager, Bookmark page, Find, Print, Save,
Full screen, Custom keybind, Stop, Open URL, Hold next/prev tab, Hold zoom in/out, Page zoom
[slider], Bookmarks wheel, History wheel.

### win-tools (W) — Win32 / WASAPI
Window: minimize, maximize, restore, close, bring to front, send to back, always-on-top,
borderless, transparency, set opacity, click-through, max/restore, move to next monitor, center,
halves, quarters, center, fullscreen, custom position, + "dynamic" wheel variants of each,
switch-window wheel. Audio: set app volume, app volume ±, app mute, app volume [slider].
System: virtual desktop prev/next/new/close, Task View, Show Desktop, Screen snip, Explorer,
Settings, Run, Emoji panel, Task Manager, System stats, Lock PC, Text snippet,
Clipboard history wheel.

### mac-tools (M) — Accessibility, CoreAudio, Dock, Text Input Sources
Window: minimize, fill, restore, close, bring to front, hide app, fill/restore, full screen,
switch-window wheel, next display, center, halves, quarters, thirds, fill display, custom
position, + dynamic wheel variants. System: prev/next Space, Mission Control, App Exposé,
Show Desktop, Spotlight, Screenshot, Emoji, Hide others, Force Quit, Lock screen, Text snippet,
Finder, System Settings, Activity Monitor, Sleep display, System stats (CPU/GPU/memory).
Audio: Mic mute [toggle], Output device wheel, Input device wheel. Clipboard history wheel.

### claude (W, M) — Claude Code session files + process tree
Sessions wheel, Remote Control, Remote Control wheel, Usage, Recent sessions wheel.

---

## 3. Per-plugin detail (top 20)

Each entry: how the Stream Deck plugin works → its actions → KnobDeck route → gaps (what to add).

### #1 Volume Controller — Elgato · W ✅ M ✅ (per-app W only)
**How it works:** native audio APIs (WASAPI sessions on Windows, CoreAudio devices on macOS). App
volume needs per-process audio sessions, which macOS does not expose (it would need a virtual audio
driver), so app control is Windows-only.
**Actions:** Profile (build a page from currently playing apps) · App volume (key/dial: set, ±, mute) ·
Input device volume · Output device volume.
**KnobDeck:** win-tools has app volume set/±/mute/slider; mac-tools has device pickers and mic mute.
**Gaps:**
- W: input/output **device volume** (set, ±, mute, slider) — `IAudioEndpointVolume`.
- M: input/output device volume and mute — CoreAudio `kAudioDevicePropertyVolumeScalar` / `…Mute`.
- W: "Profile from current apps" → a dynamic wheel/page of active audio sessions (fits our dynamic wheel).
- M: per-app volume — **not feasible** without a driver; leave out (Elgato doesn't support it either).

### #2 OBS Studio — Elgato · W ✅ M ✅
**How it works:** obs-websocket (built into OBS 28+), live state on keys.
**Actions:** Record · Record Pause · Stream · Replay Buffer · Save Replay · Scene Collection · Scene ·
Source (show/hide) · Mute (source audio) · Media (playback) · Studio Mode · Preview Scene (push live) ·
Filter · Screenshot · Transition · Chapter Marker. Dials: source volume, etc.
**KnobDeck:** `obs` has all 16 plus Virtual Cam and Profile.
**Gaps:** source **volume dial/slider** (check `audio-mixer` covers only mute). Otherwise none.

### #3 Discord — Elgato · W ✅ M ✅
**How it works:** Discord desktop RPC over IPC with OAuth.
**Actions:** Mute · Deafen · Voice Channel · Text Channel · Push to Talk · Push to Mute · Video Toggle ·
Stream Toggle · (15 total incl. volume, audio device, soundboard).
**KnobDeck:** `discord` covers all and adds wheels, DM, activity, notifications. **Gaps:** none known.

### #4 Voicemod — Voicemod · W ✅ M ✅ *(partial action list)*
**How it works:** talks to the Voicemod desktop app over its local **Control API** (websocket on
localhost; needs an API key from Voicemod's developer programme — check licence terms before building).
**Actions:** Select voice (per key, with voice image) · Random voice · Toggle voice changer on/off ·
Toggle "hear myself" · Toggle background effects · Play sound / meme · Stop all sounds ·
Mute mic · Bleep/censor (hold).
**KnobDeck route:** app's local websocket (Rule 1 tier 3). Same code on W and M.
**Helper needed:** no (Voicemod app is the endpoint). **Account:** Voicemod API key (ours, as developer).

### #5 Spotify Integration — BarRaider · W ✅ M ❌ *(partial action list)*
**How it works:** Spotify **Web API** with the user's own developer app (Client ID/secret, redirect
`http://127.0.0.1:4202`); **Premium required** for playback control.
**Actions:** Play/Pause (device picker) · Next/Previous · Shuffle · Repeat · Volume set/± (dial) ·
Mute · Like/Save track · Play URI (playlist/album/track) · Add to playlist · Now-playing display.
**Adapt:** merge into one `spotify` plugin with #8 (below).

### #6 Control Center — Elgato · W ✅ M ✅
**How it works:** Elgato Key Light / Ring Light / Light Strip expose a local REST API
(`http://<light>:9123/elgato/lights`), found by mDNS (`_elg._tcp`). Control Center app not required
for the API itself.
**Actions:** Power (one/all) · Set brightness · Adjust brightness · Set temperature · Adjust
temperature · Set colour (Light Strip) · Battery (Key Light Mini). Dials: brightness, temperature
(press = on/off).
**KnobDeck route:** HTTP to the light, mDNS discovery. Pure Rust, same on W and M. No helper.
Natural fit for KnobDeck sliders/dials.

### #7 Twitch — Elgato · W ✅ M ✅  ·  #17 Twitch Tools — BarRaider · W ✅ M ❌
**How it works:** Twitch Helix REST API + chat (IRC/websocket) + EventSub, OAuth sign-in.
**Actions (#7):** Chat message · Clear chat · Emote-only · Follower-only · Sub-only · Slow mode ·
Ad break · Stream info (title/category) · Marker · Viewer count · Create clip · Open clip.
**Actions (#17, partial):** Giveaways · Clip-to-chat · Send chat message · Ban user · Update title/tags ·
Twitch Pager (flash on follow/sub/bits/channel points or `!page`) · Channel Monitor (favourite streamers live).
**Adapt:** one `twitch` plugin (W, M). Needs our own Twitch app client id; token exchange like
Discord (code → KnobDeck website, never ship a secret). Alerts map to KnobDeck notifications.

### #8 Spotify — Elgato · W ✅ M ✅ (macOS 12+)
**How it works:** basic playback via the **Spotify desktop app**; library/search/queue/playlists need
Spotify sign-in (Web API).
**Actions:** Playback (play/pause/toggle/prev/next) · Shuffle · Repeat (cycle) · Volume (step) ·
Save to Liked Songs · Add to playlist · Play Collection (playlists, liked, albums, podcasts, top
tracks…) · Saved Search · Queue · Sleep Timer. Dials: Multimedia browser, Play Collection,
Saved Search, Queue. Infobar: Now Playing.
**KnobDeck route for `spotify` (merges #5, #8):**
- M: AppleScript `tell application "Spotify"` (play/pause, next, prev, volume, shuffle, repeat,
  current track) — no account, no Premium. Library features via Web API.
- W: no scripting interface; now-playing and play/pause/next/prev via **SMTC**
  (`GlobalSystemMediaTransportControlsSessionManager`, WinRT) — no account. Shuffle/repeat/volume/
  library via Web API (Premium for playback commands).
- Web API: OAuth PKCE (no secret to ship). Spotify's dev-mode limit is 25 users until the app is
  approved — apply early, or let users bring their own client id like BarRaider.
**Helper:** none.

### #9 YouTube — Elgato · W ✅ M ✅
**How it works:** YouTube Data / Live Streaming API, Google OAuth.
**Actions:** Viewers (live count) · Chat message · Play ad · Open dashboard · Start/Stop stream.
**KnobDeck route:** Google APIs over HTTP, same on both OSes. **Risk:** Google OAuth app
verification for the `youtube` scope and API quota — plan time for it. Lower priority than Twitch.

### #10 Audio Switcher — Fred Emmott · W ✅ M ✅ (open source)
**How it works:** Windows: undocumented `IPolicyConfig` COM to set the default endpoint
(long-stable, used by every switcher); macOS: CoreAudio default-device properties.
**Actions (manifest):** Toggle Audio Device (between two) · Set Audio Device. Options: input/output,
default vs communications role.
**KnobDeck:** mac-tools has Output/Input device wheels. **Gaps:** W — device switching is missing
from win-tools (set, toggle-between-two, wheel). M — add "Set device" key and "Toggle two" key.

### #11 Win Tools — BarRaider · W ✅ *(partial action list)*
**Actions:** App/device volume · Bluetooth on/off · Wi-Fi on/off · Explorer Scratch Pad (cycle common
folders) · Multi-Clip (each key a separate clipboard slot) · Ping (latency on key) · Power plan switch ·
Windows services start/stop/restart · Virtual desktops.
**KnobDeck:** win-tools has app volume, virtual desktops (keystroke), clipboard history wheel.
**Gaps (W):** Bluetooth / Wi-Fi radio toggle (WinRT `Windows.Devices.Radios`), power plan
(`PowerSetActiveScheme`), services (SCM API, needs elevation — note UAC), ping (ICMP `IcmpSendEcho`),
Multi-Clip slots, folder scratch pad (fits Open path + wheel).
**macOS twins for mac-tools:** Wi-Fi (CoreWLAN), Bluetooth (no public on/off API — skip or IOBluetooth
private), Low Power Mode (`pmset`, needs admin), ping, multi-clip.

### #12 OBS Tools — BarRaider · W ✅ M ❌
**How it works:** obs-websocket.
**Actions:** Smart scene switcher (preview/live borders) · Source animation (multi-phase moves) ·
Studio mode toggle · Video player (switch file/speed) · Source volume (set/±) · Dropped-frames alarm ·
OBS CPU usage · Previous scene · Twitch integration (`!replay`, clips) · Browser sources (URL/refresh) ·
Remote recording toggle (other PC) · Set transition · Filter toggle · Instant replay (save + play back).
**Gaps for `obs`:** previous scene, source volume set/± and slider, stats keys (CPU, dropped frames,
FPS — `GetStats`), browser source refresh / set URL, preview/program border state on scene keys,
multiple OBS connections (remote PC), instant replay playback. Source animation: later.

### #13 Clocks — Elgato · W ✅ M ✅
**Actions:** My Clock (analog/digital, 30+ styles, 12/24 h, date) · World Clock (time zones,
dial to switch) · Countdown timer.
**KnobDeck route:** pure Rust (`chrono`/`chrono-tz`), rendered key image. Could be host built-ins
instead of a plugin. Also add a Stopwatch/Timer (Stream Deck has a built-in Timer action).

### #14 Wave Link (Legacy) — Elgato · W ✅ M ✅
**How it works:** Wave Link app's local websocket API (Wave Link 2; v3 uses a newer plugin, #26).
**Actions:** Input (set/±/mute per mix) · Output (set/±/mute, switch mix) · Device (gain, monitor
mix, output switch) · Effects (toggle effect or chain). Dials for each.
**Decision:** defer — only useful with Elgato Wave mics; Legacy is frozen. If built, target Wave Link 3.

### #15 SuperMacro — BarRaider · W ✅ M ❌
**How it works:** SendInput keyboard + mouse simulation with a macro language.
**Actions:** Super Macro · Super Macro Toggle (two macros) · Sticky Super Macro (repeat until pressed
again) · Keystroke PTT (repeat while held) · Sticky Keystroke · Mouse Location (shows X,Y).
Language: `{{ctrl}{c}}` chords, `{{PAUSE:ms}}`, mouse move/click/scroll, variables, functions, loops,
read from file.
**KnobDeck:** host Keybind + Multi + Delay + Text snippet cover simple macros.
**Gaps (host, both OSes):** mouse move/click/scroll steps, repeat-while-held, repeat-until-pressed,
text with variables (date, clipboard). Allowed under Rule 1 because the action *is* input.
macOS needs Accessibility permission (already required by mac-tools).

### #16 Audio Mute — Fred Emmott · W ✅ M ✅ (open source)
**Actions (manifest):** Toggle Mute · Mute · Unmute — any input or output device (default or chosen).
Key shows muted state.
**KnobDeck:** mac-tools Mic mute toggle. **Gaps:** W — device mute toggle/mute/unmute in win-tools
(`IAudioEndpointVolume::SetMute`, with state feedback). M — pick a specific device and output mute;
explicit Mute / Unmute (for multi-actions). Add push-to-talk (hold) on both.

### #18 Weather — Elgato · W ✅ M ✅
**Actions:** Weather (current temp/conditions, press = high/low; dial = hourly/daily/chart) ·
Forecast (multi-day) · Air Quality (AQI).
**KnobDeck route:** Open-Meteo (free, no key, has AQI) over HTTP; location by city search or
coordinates. Same on both OSes. No helper.

### #19 Streamlabs — Streamlabs · W ✅ M ✅ *(partial action list)*
**How it works:** Streamlabs Desktop's local websocket/JSON-RPC API (token from Settings ▸ Remote Control).
**Actions:** Go live / end stream · Start/stop recording · Switch scene · Toggle source · Mute audio
source · Save replay · Alerts (skip, pause/unpause, mute alert sound).
**KnobDeck route:** local API, both OSes; very close to our OBS plugin's shape — share helpers.
(#31 "Streamlabs Desktop" is the newer listing of the same app; build one plugin.)

### #20 Windows Mover & Resizer — BarRaider · W ✅ M ❌
**Actions:** Windows Mover — target focused window **or a specific app**, choose screen, position
(capture current), size, maximize/minimize; dial version; virtual-desktop move.
**KnobDeck:** win-tools / mac-tools have positions, monitors, custom position, dynamic wheels.
**Gaps:** "specific application" target (by exe / bundle id) on fixed keys; "capture current window
position" button in the custom-position input; move window to another virtual desktop/Space (no
public API on either OS — skip).

---

## 4. Stream Deck built-in actions vs KnobDeck host

| Stream Deck built-in | KnobDeck |
|---|---|
| Hotkey, Open (app/file), Website, Multimedia, Text | ✅ Keybind, Launch / Open path, Open URL, media keys, Text snippet (win/mac-tools) |
| Multi Action, Multi Action Switch, Create Folder | ✅ Multi, Multi-toggle, Folder |
| Hotkey Switch (two hotkeys on one key) | ✅ via Multi-toggle |
| Delay (inside multi action) | ✅ Delay |
| Switch Profile, Previous/Next Page, Go to Page, Page Indicator | check — Link kind covers navigation; confirm page/profile switching |
| Random Action | ❌ add (host) |
| Timer / Stopwatch | ❌ add (with Clocks) |
| Soundboard (play audio file, stop) | ❌ add (host or small plugin; play to a chosen output device) |
| Brightness / Sleep of the device | n/a (phone/dial device; check for screen-dim action) |

---

## 5. Helper extensions / companion apps to build and maintain

A KnobDeck plugin alone cannot drive an app that has no external interface. Then we ship a helper that
runs *inside* the app and exposes a local endpoint.

| Helper | Status | Used by | Ships to | Notes |
|---|---|---|---|---|
| KnobDeck VS Code Companion (extension) | **exists** (source not in this workspace) | `vscode` | VS Code Marketplace + Open VSX (for Cursor, VSCodium, Windsurf) | Local HTTP action server, host/port/password. Must track VS Code API changes. |
| KnobDeck IntelliJ Companion (IDE plugin) | **exists** (source not in this workspace) | `intellij` | JetBrains Marketplace | Kotlin. Rebuild/verify for each yearly IDE platform (`sinceBuild`/`untilBuild`). |
| Browser extension (WebExtension) + native messaging host | **proposed** | `browser-plugin` | Chrome Web Store, Edge Add-ons, Firefox AMO | Removes keystrokes on Windows and Firefox/Zen: tabs, zoom, reload, find via `chrome.tabs` API. One extension codebase, 3 stores. |
| — | not needed | OBS, Discord, VLC, Zoom, Voicemod, Streamlabs, Wave Link | — | The app already has a local API (enable it in the app's settings). |
| — | not needed | Spotify, Twitch, YouTube, Weather, Elgato lights | — | Web/LAN APIs. Need our OAuth apps (Spotify, Twitch, Google) and a token-exchange endpoint on the KnobDeck website, as Discord has. |

Out of the top 20 but likely next, and **helper-required**: Adobe Photoshop / Premiere / Lightroom
(UXP plugin), Microsoft Office/Teams (Office add-in or Graph API), DaVinci Resolve (scripting API,
Studio only on some features). Plan a helper repo per host app, each with its own store release.

Maintenance cost per helper: one more store listing, signing, review cycle and version-compat matrix.
Keep the protocol (HTTP + password, same as vscode/intellij) identical across helpers so the Rust side
reuses one client.

---

## 6. OS support summary for adaptations

| KnobDeck target plugin | Windows route | macOS route | Ship |
|---|---|---|---|
| win-tools additions (device volume/mute/switch, radios, power plan, services, ping, multi-clip) | WASAPI, `IPolicyConfig`, WinRT Radios, PowerProf, SCM, ICMP | — | `win-tools` |
| mac-tools additions (device volume/mute, set/toggle device, Wi-Fi, ping, multi-clip) | — | CoreAudio, CoreWLAN | `mac-tools` |
| obs additions | obs-websocket | same | `obs-windows`, `obs-macos` |
| voicemod | local websocket | same | both |
| spotify | SMTC + Web API | AppleScript + Web API | both (different control seam) |
| elgato-lights | HTTP + mDNS | same | both |
| twitch | Helix + chat | same | both |
| youtube | Google APIs | same | both |
| clocks / timer / weather | pure Rust + Open-Meteo | same | both (or host) |
| streamlabs | local API | same | both |
| host: macro steps, random action, soundboard, timer | SendInput, audio playback | CGEvent, AVAudio/CoreAudio | host |

---

## 7. Progress

| Step | Status |
|---|---|
| win-tools 1.2.0: audio devices (#1, #10, #16) — Mute Microphone, Push to Talk, Output/Input Device wheels, Set Audio Device, Toggle Audio Device, Mute Device, Device Volume | done (switching between two different devices not yet tried on a machine with two) |
| obs 1.1.0 (#2, #12): Previous Scene, Preview Scene, Source Volume dial, OBS Stats, Browser Source refresh/URL | done (tested against a scripted obs-websocket server, not a live OBS). Not done: source animation, instant-replay playback, second OBS on another PC |
| win-tools 1.3.0 (#11): Wi-Fi / Bluetooth toggle, Power Plan key + wheel, Ping | done (radios read, not switched, in tests). Not done: Windows services (needs admin), Multi-Clip (clipboard history covers it) |
| win-tools 1.4.0 (#20): Move App Window — a named app's window to any position/monitor, thirds, custom bounds | done. Not done: moving a window to another virtual desktop (no public API) |
| SDK 0.4.0: `save_env`, `open_url`, `oauth` feature (PKCE loopback, device code, token refresh); `live` may push state | done, pushed |
| elgato-lights 1.0.0 (#6): Power, Brightness, Temperature, Set Brightness/Temperature, Set Color, Battery; mDNS discovery | done (tested against mock lights; no real light here) |
| weather 1.0.0 (#18): Weather, Forecast wheel, Hourly wheel, Air Quality — Open-Meteo, no key | done (tested against the live API) |
| clocks 1.0.0 (#13 + Stream Deck's Timer): Clock (any zone), World Clock wheel, Countdown Timer, Stopwatch | done |
| streamlabs 1.0.0 (#19, #31): Go Live, Record, Replay Buffer, Save Replay, Scene, Source Visibility, Mute, Volume | done (tested against a scripted API server built from Streamlabs' docs; not against the real app) |
| voicemod 1.0.0 (#4): Voice Changer, Hear Myself, Background Effects, Mute Mic, Mute Sounds for Me, Select/Random Voice, Favorites wheel, Play/Stop Sound, Bleep | done (tested against a scripted Control API server; needs the VOICEMOD_CLIENT_KEY repo variable or a user key) |
| spotify 1.0.0 (#5, #8): Play/Pause, Next, Previous, Now Playing via the OS; Like, Shuffle, Repeat, Volume, Play Playlist, Playlists wheel, Add to Playlist via the Web API | done (Web API tested against a mock incl. token refresh; SMTC read path tested; Spotify not installed here; needs the SPOTIFY_CLIENT_ID repo variable). Not done: queue, saved search, sleep timer |
| twitch 1.0.0 (#7, #17): Chat Message, Clear Chat, Emote/Follower/Sub-Only, Slow Mode, Run Ad, Stream Info, Marker, Create Clip, Viewer Count, Channel Monitor | done (tested against a mock Helix; device sign-in not run; needs the TWITCH_CLIENT_ID repo variable). Not done: Pager (EventSub), giveaways, bans, open latest clip |
| youtube 1.0.0 (#9): Viewers, Chat Message, Play Ad, Live Dashboard, Go Live / End Stream | done (tested against a mock API; Google sign-in not run). Needs the GOOGLE_CLIENT_ID variable, a KnobDeck website token endpoint (KNOBDECK_GOOGLE_TOKEN_URL) that adds the secret, and Google's OAuth verification for youtube.force-ssl |
| spotify 1.0.1: Web API actions and setup commented out until KnobDeck's Spotify app (owner needs Premium) and SPOTIFY_CLIENT_ID exist; OS-route keys need no setup | done |
| youtube 1.0.1: read-only sign-in (youtube.readonly); Chat Message, Play Ad, Go Live commented out (privacy; OBS starts streams) | done |
| twitch 1.0.1: rebuilt with TWITCH_CLIENT_ID | done |
| Deferred: Voicemod client key, Wave Link (#14, hardware-specific), host actions (Random Action, Soundboard), mac-tools twins of the win-tools device/radio/ping actions (need a Mac to build and test) | — |

## 8. Suggested order

1. **Fill gaps in what we already ship** (cheap, high value): Windows device volume/mute/switch
   (#1, #10, #16), OBS extras (#2, #12), Win Tools extras (#11), mover "specific app" (#20).
2. **Spotify** (#5 + #8) — two of the top 8.
3. **Elgato Lights** (#6), **Weather** (#18), **Clocks/Timer** (#13) — no accounts, quick wins.
4. **Twitch** (#7 + #17), **Voicemod** (#4), **Streamlabs** (#19).
5. **YouTube** (#9) — after Google verification. **Wave Link 3** (#26) if users ask.
6. Host: macro steps, random action, soundboard (#15, built-ins).

---

## Appendix: ranks 21–40 (next candidates)

21 Speed Test (BarRaider, W) · 22 Advanced Launcher (BarRaider, W) · 23 Discord Volume Mixer (Danol, W) ·
24 ShadowPlay (BarRaider, W) · 25 Sound Deck (GeekyEggo, W) · 26 Wave Link 3 (Elgato, W/M) ·
27 iCUE (Corsair, W/M) · 28 System Vitals (VIVRE-MOTION, W) · 29 Home Assistant (C. Giesche, W/M) ·
30 Camera Hub (Elgato, W/M) · 31 Streamlabs Desktop (Streamlabs, W/M) · 32 Window Mover (Elgato, W/M) ·
33 TikTok LIVE Studio (W) · 34 Battery (BarRaider, W) · 35 Stream Counter (BarRaider, W) ·
36 Philips Hue (Elgato, W/M) · 37 Audio Mixer (J. Poirier, W) · 38 YouTube Ticker (W/M) ·
39 Govee Controller (GeekyEggo, W/M) · 40 Advanced Screen Saver (BarRaider, W).

Notable: **Home Assistant** (#29) and **Philips Hue** (#36) are cheap, popular, cross-platform network
plugins; **System Vitals** (#28) overlaps our System stats.

Re-run the ranking with:
`curl "https://mp-gateway.elgato.com/products?extension=plugins&limit=40&ordering=-download_count"`
