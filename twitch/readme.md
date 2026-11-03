# Twitch

Run your Twitch channel from KnobDeck, on Windows and macOS.

## What it does

- Chat Message (posts as you), Clear Chat.
- Emote-Only, Follower-Only, Subscriber-Only and Slow Mode toggles that follow the chat's settings.
- Run Ad (30 s – 3 min), Stream Info (title and/or category), Stream Marker.
- Create Clip (optionally opens the clip editor), Viewer Count on a key.
- Channel Monitor: shows whether a streamer is live, with their viewer count; press to open the channel.

## Setup

Setup opens twitch.tv/activate with a code already filled in; approve KnobDeck there. If you
registered your own app at dev.twitch.tv (client type **Public**), paste its client ID in setup.

## How it works

The Twitch Helix API with your sign-in (device-code flow; no password or secret is stored, only a
token in KnobDeck's plugin data folder).
