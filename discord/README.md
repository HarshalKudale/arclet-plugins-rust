# Discord Plugin for Arclet

Discord integration plugin — control mute, deafen, push-to-talk, channels, volume, soundboard, and more directly from your Arclet hub.

## Features

| Action | Type | Description |
|--------|------|-------------|
| **Mute** | Toggle | Toggle microphone mute |
| **Deafen** | Toggle | Toggle headphone deafen |
| **Push to Talk** | Hold | Hold to transmit |
| **Push to Mute** | Hold | Hold to mute |
| **Voice Mode Toggle** | Toggle | Switch between Voice Activity ↔ Push-to-Talk mode |
| **Voice Channel** | Toggle | Join / leave a voice channel |
| **Text Channel** | Button | Navigate to a text channel |
| **Server Jump** | Button | Jump to a server without manually finding channel IDs |
| **Open Direct Message** | Button | Open a DM from a friend picker |
| **Video Toggle** | Toggle | Turn camera on/off in calls |
| **Stream Toggle** | Toggle | Start/stop screen sharing |
| **Volume Control** | Button | Adjust input/output device volume |
| **User Volume Control** | Button | Adjust per-user local volume |
| **Set Audio Device** | Button | Switch input/output audio device |
| **Soundboard** | Button | Play a Discord Soundboard sound |
| **Voice Channel Wheel** | Dynamic Wheel | Pick a voice channel from a server live |
| **Call Member Mute Wheel** | Dynamic Wheel | Mute / unmute current call members without user IDs |
| **Activity Preset** | Button | Set or clear Discord activity text |
| **Soundboard Wheel** | Dynamic Wheel | Trigger sounds from a live sound picker |
| **Notifications** | Toggle | Show & manage unread notifications |
| **Server Stats** | Button | Display online user / message counts |

---

Most Discord actions now prefer **server, channel, friend, device, sound, and call-member pickers** over raw guild/channel/user IDs.

---

## Setup

### 1. Create a Discord Developer App

1. Go to [https://discord.com/developers/applications](https://discord.com/developers/applications)
2. Click **New Application** → name it `Arclet Discord` (or anything you like)
3. Note your **Application ID** (`CLIENT_ID`) on the General Information page
4. Go to **OAuth2** → copy the **Client Secret** (`CLIENT_SECRET`)
5. Under **OAuth2 → Redirects** add: `http://127.0.0.1:31416/callback`
6. Go to **Rich Presence → Art Assets** — enable RPC (the plugin uses Discord's local RPC)

### 2. Configure the Plugin

Create `discord-config.json` in the plugin's folder (next to `plugin.js`):

```json
{
  "clientId":     "YOUR_APPLICATION_ID",
  "clientSecret": "YOUR_CLIENT_SECRET"
}
```

Or set environment variables `DISCORD_CLIENT_ID` and `DISCORD_CLIENT_SECRET`.

### 3. Install

```sh
npm install
npm run dev -- --install
```

Restart Arclet — the Discord actions will appear in the palette.

### 4. Authorize

When you first press any Discord action, a browser window will open asking you to authorize
the app with the required permissions. After authorizing, the plugin stores the token and
reconnects automatically.

---

## Required OAuth2 Scopes

The plugin requests the following scopes during authorization:

| Scope | Purpose |
|-------|---------|
| `identify` | Identify the current user for RPC auth |
| `rpc` | Base RPC connection to Discord |
| `rpc.voice.read` | Read voice settings and channel state |
| `rpc.voice.write` | Control mute/deafen/PTT/volume |
| `guilds` | List servers for channel pickers |
| `messages.read` | Server stats and notifications |

---

## Discord App ID & Public Key

The registered Discord Developer App details used in this plugin:

- **Application ID**: `1527677329524916424`
- **Public Key**: `c5e58f44f780eff05911c52c387b9bd111e409cae85b47711a580a5046011aa9`

---

## Development

```sh
# Run in dev mode (no compilation, uses node directly):
npm run dev -- --install

# Run WS protocol smoke test:
npm run dev -- --test

# Build distributable .arclet:
npm run build

# Run unit tests:
npm test
```

---

## Architecture

```
plugin.js           Main entry — definePlugin() + all actions
discord-rpc.js      Discord IPC client (named pipe \\.\pipe\discord-ipc-{0-9})
oauth.js            OAuth2 authorization code + PKCE + token refresh
state.js            Shared live voice state (muted, deafened, channel, etc.)
rest.js             Discord REST API helpers (guilds, channels, soundboard)
```

The plugin connects to the **Discord desktop client** via its local named-pipe RPC (same protocol
used by Discord overlays and rich presence apps). No browser or web scraping involved — this is
the official Discord RPC API.
