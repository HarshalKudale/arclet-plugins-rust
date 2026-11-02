// Decides what to build: a plugin is built for an OS when the `version` in its Cargo.toml differs
// from the one in the previous release's <dir>_<os>_manifest.json (or there is none). Everything
// else is carried over from that release by reuse.mjs. Bump the version in Cargo.toml to ship.
//   node scripts/plan.mjs <dir with the previous release's manifests>   -> GitHub Actions matrix JSON
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const prevDir = process.argv[2];
const OS_PLUGINS = {
  windows: ["browser-plugin", "claude", "discord", "intellij", "obs", "vlc", "vscode", "win-tools", "zoom"],
  macos: ["browser-plugin", "claude", "discord", "intellij", "mac-tools", "obs", "vlc", "vscode", "zoom"],
};
const RUNNER = { windows: "windows-latest", macos: "macos-latest" };

const include = Object.entries(OS_PLUGINS).flatMap(([name, dirs]) => {
  const todo = dirs.filter((dir) => {
    const version = readFileSync(`${dir}/Cargo.toml`, "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    if (!version) throw new Error(`${dir}/Cargo.toml has no version`);
    const prev = join(prevDir, `${dir}_${name}_manifest.json`);
    return !existsSync(prev) || JSON.parse(readFileSync(prev, "utf8")).version !== version;
  });
  return todo.length ? [{ os: RUNNER[name], name, plugins: todo.join(" ") }] : [];
});
console.log(JSON.stringify({ include }));
