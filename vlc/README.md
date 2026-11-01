# VLC Plugin for KnobDeck

VLC integration plugin for KnobDeck — control playback over VLC's built-in **HTTP interface**, so VLC does **not** need to be focused.

## Supported actions

- Play / Pause
- Stop
- Next / Previous
- Seek backward / forward 10 seconds
- Fullscreen toggle
- Volume up / down
- Open stream / media URL
- Playback status indicator

## Setup

1. Open **VLC**
2. Go to **Tools → Preferences**
3. Switch **Show Settings** to **All**
4. Open **Interface → Main interfaces**
5. Enable **Web**
6. Open **Main interfaces → Lua**
7. Set a **Lua HTTP password**
8. Save and restart VLC
9. Run the KnobDeck plugin setup and enter:
   - Host: usually `127.0.0.1`
   - Port: usually `8080`
   - Password: the Lua HTTP password you set

The plugin validates the HTTP interface during setup before saving the config.
