# Elgato Lights

Control Elgato Key Light, Key Light Air, Key Light Mini, Ring Light and Light Strip from KnobDeck.
Works on Windows and macOS, with or without Elgato Control Center running.

## What it does

- Power: turn one light or all lights on and off. The key follows the light, even when it is
  switched from Control Center or its button.
- Brightness and Temperature on a dial or slider (2900–7000 K), plus keys that set a fixed value.
- Set Color for a Light Strip (`#RRGGBB`).
- Battery level of a Key Light Mini on a key.

## Setup

Lights on the same network are found automatically (mDNS). Setup checks that at least one light
answers; if a light is not found (for example on a network that blocks multicast), type its
address, shown in Control Center's light settings.

## How it works

Each light has a local HTTP API on port 9123 (`/elgato/lights`); the plugin reads and changes it
directly, so no keyboard shortcuts are involved. Colour temperature is converted from Kelvin to the
light's own units.
