// Decides what to build. index.json lists every plugin's version; you change it together with the
// plugin's Cargo.toml when you ship a plugin. A plugin is built for an OS when its version in
// index.json differs from the one in the previous release's registry.json (or it is not there).
// Everything else is carried over from that release by registry.mjs.
//   node scripts/plan.mjs [previous registry.json]   -> GitHub Actions matrix JSON
import { existsSync, readFileSync } from "node:fs";
import { OS_PLUGINS, findEntry, readIndex } from "./plugins.mjs";

const prevPath = process.argv[2];
const prev = prevPath && existsSync(prevPath) ? JSON.parse(readFileSync(prevPath, "utf8")).plugins : [];
const index = readIndex();
const RUNNER = { windows: "windows-latest", macos: "macos-latest" };

// The binary reports its Cargo.toml version, so the two must agree.
for (const dir of new Set(Object.values(OS_PLUGINS).flat())) {
  const cargo = readFileSync(`${dir}/Cargo.toml`, "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (index[dir]?.version !== cargo) throw new Error(`${dir}: index.json says ${index[dir]?.version}, ${dir}/Cargo.toml says ${cargo}`);
}

const include = Object.entries(OS_PLUGINS).flatMap(([name, dirs]) => {
  const todo = dirs.filter((dir) => findEntry(prev, dir, name)?.version !== index[dir].version);
  return todo.length ? [{ os: RUNNER[name], name, plugins: todo.join(" ") }] : [];
});
console.log(JSON.stringify({ include }));
