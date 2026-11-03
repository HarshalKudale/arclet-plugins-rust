# Discord Plugin for KnobDeck

Discord integration plugin — control mute, deafen, push-to-talk, channels, volume, soundboard, and more directly from your KnobDeck hub.

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

1. Open the **Discord desktop app** and sign in.
2. In KnobDeck, open **Plugins → Discord** and click **Setup**.
3. Discord shows an authorization popup inside the client. Click **Authorize**.

That's it. KnobDeck stores the sign-in in its plugin data folder (`%APPDATA%\KnobDeck\plugin-data\discord\` on
Windows, `~/Library/Application Support/KnobDeck/plugin-data/discord/` on macOS) and renews it on its own,
so you only run setup again if you revoke access in Discord (**User Settings → Authorized Apps**).

There is no Discord developer app or client secret to create: the token exchange happens on
`app.knobdeck.harshalkudale.com`, so the secret never ships with the plugin.

---

## Permissions

Setup asks Discord for these scopes:

| Scope | Purpose |
|-------|---------|
| `identify` | Identify the current user for RPC auth |
| `rpc` | Base RPC connection to Discord |
| `rpc.voice.read` | Read voice settings and channel state |
| `rpc.voice.write` | Control mute, deafen, push-to-talk and volume |
| `guilds` | List servers for channel pickers |
| `messages.read` | Server stats and notifications |

---

## Troubleshooting

- **"Discord is not connected"**: Discord must be running. The plugin reconnects on its own when it starts.
- **Setup fails**: make sure you are signed in to the Discord desktop app (not only the browser) and try again.
- **Actions stop working after a while**: run **Setup** again; this happens when access was revoked in Discord.

The plugin talks to the Discord desktop client over its local RPC pipe, the same channel Discord's
own overlays use. Nothing is scraped and no browser is involved.
