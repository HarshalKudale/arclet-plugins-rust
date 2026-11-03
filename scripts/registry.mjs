// Builds out/registry.json for a release: the plugins built in this run (out/part-<os>.json) plus
// every other plugin carried over from the previous release (its bundle and readme are
// downloaded into out/ so the new release is complete). The website copies registry.json as is.
//   node scripts/registry.mjs <new tag> [<previous tag> <previous registry.json>]
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { OS_PLUGINS, findEntry, readIndex } from "./plugins.mjs";

const [tag, prevTag, prevPath] = process.argv.slice(2);
const REPO = "https://github.com/HarshalKudale/knobdeck-plugins";
// Registry site (knobdeck-app-site repo): serves registry.json and the readmes.
const SITE = "https://app.knobdeck.harshalkudale.com";
// Icons live in the repo and are hot-linked from here, so changing one needs no release.
const RAW = "https://raw.githubusercontent.com/HarshalKudale/knobdeck-plugins/master";
const index = readIndex();
const prev = prevPath ? JSON.parse(readFileSync(prevPath, "utf8")).plugins : [];

const built = [];
for (const f of readdirSync("out").filter((f) => /^part-.*\.json$/.test(f))) {
  built.push(...JSON.parse(readFileSync(`out/${f}`, "utf8")));
  rmSync(`out/${f}`);
}

const entries = [...built];
for (const [os, dirs] of Object.entries(OS_PLUGINS)) {
  for (const dir of dirs) {
    if (built.some((e) => e.dir === dir && e.platforms.includes(os))) continue;
    const old = findEntry(prev, dir, os);
    if (!old) throw new Error(`${dir} (${os}) was neither built nor in the previous release`);
    const files = [old.file, old.readmeFile].filter(Boolean).flatMap((f) => ["-p", f]);
    execFileSync("gh", ["release", "download", prevTag, ...files, "-D", "out", "--clobber"], { stdio: "inherit" });
    const { iconFile, ...rest } = old; // releases made before icons moved into the repo carried one
    entries.push({ ...rest, dir });
    console.log(`reused ${old.id} ${old.version}`);
  }
}

/** The manifest's icon as a file in <dir>/icons (svg or png) linked from the repo, else null (e.g. a Lucide name). */
const iconUrl = (dir, icon) =>
  icon && /\.(svg|png)$/i.test(icon) && existsSync(`${dir}/icons/${icon}`) ? `${RAW}/${dir}/icons/${encodeURIComponent(icon)}` : null;

const plugins = entries
  .map(({ dir, ...e }) => {
    const meta = index[dir];
    const base = `${SITE}/plugins/${e.id}`;
    return {
      id: e.id,
      name: meta.name,
      summary: meta.summary ?? e.description,
      description: e.description,
      version: e.version,
      author: e.author,
      repositoryUrl: REPO,
      artifactUrl: `${REPO}/releases/download/${tag}/${e.file}`,
      iconUrl: iconUrl(dir, e.icon),
      icon: e.icon,
      readmeUrl: e.readmeFile ? `${base}/readme.md` : null,
      sha256: e.sha256,
      tags: meta.tags ?? [],
      platforms: e.platforms,
      featured: meta.featured ?? false,
      // Release asset names: the bundle, and the readme the website copies to readmeUrl.
      file: e.file,
      readmeFile: e.readmeFile,
      sort: meta.sort ?? 1000,
    };
  })
  .sort((a, b) => Number(b.featured) - Number(a.featured) || a.sort - b.sort || a.name.localeCompare(b.name) || a.id.localeCompare(b.id))
  .map(({ sort, ...p }) => p);
writeFileSync("out/registry.json", JSON.stringify({ tag, plugins }, null, 2) + "\n");
console.log(`out/registry.json: ${plugins.length} plugin(s)`);
