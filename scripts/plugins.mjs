// Shared by plan.mjs and registry.mjs.
import { readFileSync } from "node:fs";

/** Plugin folders that ship on each OS. */
export const OS_PLUGINS = {
  windows: ["browser-plugin", "claude", "clocks", "discord", "elgato-lights", "intellij", "obs", "spotify", "streamlabs", "twitch", "vlc", "voicemod", "vscode", "weather", "win-tools", "zoom"],
  macos: ["browser-plugin", "claude", "clocks", "discord", "elgato-lights", "intellij", "mac-tools", "obs", "spotify", "streamlabs", "twitch", "vlc", "voicemod", "vscode", "weather", "zoom"],
};

/** index.json: { plugins: { "<dir>": { version, name, summary, tags, featured, sort } } } */
export const readIndex = () => JSON.parse(readFileSync("index.json", "utf8")).plugins;

/** The registry entry of `dir` on `os` in a registry.json plugin list (the id is the dir, or dir-os for cross-platform plugins). */
export const findEntry = (entries, dir, os) => entries.find((e) => e.id === dir || e.id === `${dir}-${os}`);
