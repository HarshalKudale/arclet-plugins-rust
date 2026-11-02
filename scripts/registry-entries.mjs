// Writes out/registry-<os>.json: one entry per bundle in out/ (id, version, file, sha256, ...).
// The website reads these from the latest GitHub Release, so a new tag updates the registry.
//   node scripts/registry-entries.mjs <os> <tag>
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [os, tag] = process.argv.slice(2);
if (!os || !tag) throw new Error("usage: registry-entries.mjs <os> <tag>");
const plugins = readdirSync(".")
  .map((dir) => join(dir, "dist"))
  .flatMap((dist) => {
    try { return readdirSync(dist).filter((f) => f.endsWith(".knobdeck")).map((f) => ({ dist, f })); } catch { return []; }
  })
  .map(({ dist, f }) => {
    const id = f.replace(/\.knobdeck$/, "");
    const m = JSON.parse(readFileSync(join(dist, id, "manifest.json"), "utf8"));
    if (!m.platforms?.includes(os)) throw new Error(`${id}: manifest platforms must include ${os}`);
    return {
      id: m.id, name: m.name, version: m.version, description: m.description ?? "", author: m.author ?? null,
      icon: m.icon ?? null, platforms: m.platforms, file: f,
      sha256: createHash("sha256").update(readFileSync(join(dist, f))).digest("hex"),
    };
  });
writeFileSync(`out/registry-${os}.json`, JSON.stringify({ tag, plugins }, null, 2));
console.log(`${plugins.length} plugin(s) -> out/registry-${os}.json`);
