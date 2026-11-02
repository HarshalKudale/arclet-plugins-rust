// Decides what to build. index.json lists every plugin's version; you change it together with the
// plugin's Cargo.toml when you ship a plugin. A plugin is built for an OS when its version in
// index.json differs from the one in the previous release's <dir>_<os>_manifest.json (or there is
// none). Everything else is carried over from that release by reuse.mjs.
//   node scripts/plan.mjs <dir with the previous release's manifests>   -> GitHub Actions matrix JSON
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const prevDir = process.argv[2];
const OS_PLUGINS = {
  windows: ["browser-plugin", "claude", "discord", "intellij", "obs", "vlc", "vscode", "win-tools", "zoom"],
  macos: ["browser-plugin", "claude", "discord", "intellij", "mac-tools", "obs", "vlc", "vscode", "zoom"],
};
const RUNNER = { windows: "windows-latest", macos: "macos-latest" };
const { plugins } = JSON.parse(readFileSync("index.json", "utf8"));

// The binary reports its Cargo.toml version, so the two must agree.
for (const dir of new Set(Object.values(OS_PLUGINS).flat())) {
  const cargo = readFileSync(`${dir}/Cargo.toml`, "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (plugins[dir] !== cargo) throw new Error(`${dir}: index.json says ${plugins[dir]}, ${dir}/Cargo.toml says ${cargo}`);
}

const include = Object.entries(OS_PLUGINS).flatMap(([name, dirs]) => {
  const todo = dirs.filter((dir) => {
    const prev = join(prevDir, `${dir}_${name}_manifest.json`);
    return !existsSync(prev) || JSON.parse(readFileSync(prev, "utf8")).version !== plugins[dir];
  });
  return todo.length ? [{ os: RUNNER[name], name, plugins: todo.join(" ") }] : [];
});
console.log(JSON.stringify({ include }));
