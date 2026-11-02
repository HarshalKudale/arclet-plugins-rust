// Copies the freshly built plugins into out/ under their release names:
//   <dir>_<os>_<version>.knobdeck, <dir>_<os>_readme.md, <dir>_<os>_icon.<svg|png>
// and writes out/part-<os>.json, the registry entries of this build (registry.mjs merges them).
//   node scripts/stage.mjs <os> <dir>...
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";

const [os, ...dirs] = process.argv.slice(2);
if (!os || !dirs.length) throw new Error("usage: stage.mjs <os> <dir>...");
mkdirSync("out", { recursive: true });
const entries = [];
for (const dir of dirs) {
  const dist = join(dir, "dist");
  const bundle = readdirSync(dist).find((f) => f.endsWith(".knobdeck"));
  const unpacked = join(dist, bundle.replace(/\.knobdeck$/, ""));
  const m = JSON.parse(readFileSync(join(unpacked, "manifest.json"), "utf8"));
  if (!m.platforms?.includes(os)) throw new Error(`${m.id}: manifest platforms must include ${os}`);

  const file = `${dir}_${os}_${m.version}.knobdeck`;
  copyFileSync(join(dist, bundle), join("out", file));

  let readmeFile = null;
  if (existsSync(join(unpacked, "readme.md"))) {
    readmeFile = `${dir}_${os}_readme.md`;
    copyFileSync(join(unpacked, "readme.md"), join("out", readmeFile));
  }
  let iconFile = null;
  const iconPath = m.icon && [join(unpacked, "icons", m.icon), join(unpacked, m.icon)].find((p) => /\.(svg|png)$/i.test(p) && existsSync(p));
  if (iconPath) {
    iconFile = `${dir}_${os}_icon${iconPath.slice(iconPath.lastIndexOf("."))}`;
    copyFileSync(iconPath, join("out", iconFile));
  }

  entries.push({
    dir, id: m.id, version: m.version, description: m.description ?? "", author: m.author ?? null,
    icon: m.icon ?? null, platforms: m.platforms, file, readmeFile, iconFile,
    sha256: createHash("sha256").update(readFileSync(join("out", file))).digest("hex"),
  });
  console.log(`${m.id} ${m.version}`);
}
writeFileSync(`out/part-${os}.json`, JSON.stringify(entries, null, 2));
