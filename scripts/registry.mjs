// Builds out/registry.json for a release: the plugins built in this run (out/part-<os>.json) plus
// every other plugin carried over from the previous release (its bundle, readme and icon are
// downloaded into out/ so the new release is complete). The website copies registry.json as is.
//   node scripts/registry.mjs <new tag> [<previous tag> <previous registry.json>]
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { OS_PLUGINS, findEntry, readIndex } from "./plugins.mjs";

const [tag, prevTag, prevPath] = process.argv.slice(2);
const REPO = "https://github.com/HarshalKudale/arclet-plugins-rust";
const SITE = "https://knobdeck.harshalkudale.com";
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
    const files = [old.file, old.readmeFile, old.iconFile].filter(Boolean).flatMap((f) => ["-p", f]);
    execFileSync("gh", ["release", "download", prevTag, ...files, "-D", "out", "--clobber"], { stdio: "inherit" });
    entries.push({ ...old, dir });
    console.log(`reused ${old.id} ${old.version}`);
  }
}

const plugins = entries
  .map(({ dir, ...e }) => {
    const meta = index[dir];
    const base = `${SITE}/registry/${e.id}`;
    return {
      id: e.id,
      name: meta.name,
      summary: meta.summary ?? e.description,
      description: e.description,
      version: e.version,
      author: e.author,
      repositoryUrl: REPO,
      artifactUrl: `${REPO}/releases/download/${tag}/${e.file}`,
      iconUrl: e.iconFile ? `${base}/icon${e.iconFile.slice(e.iconFile.lastIndexOf("."))}` : null,
      icon: e.icon,
      readmeUrl: e.readmeFile ? `${base}/readme.md` : null,
      sha256: e.sha256,
      tags: meta.tags ?? [],
      platforms: e.platforms,
      featured: meta.featured ?? false,
      // Release asset names: the bundle, and the readme / icon the website copies to readmeUrl / iconUrl.
      file: e.file,
      readmeFile: e.readmeFile,
      iconFile: e.iconFile,
      sort: meta.sort ?? 1000,
    };
  })
  .sort((a, b) => Number(b.featured) - Number(a.featured) || a.sort - b.sort || a.name.localeCompare(b.name) || a.id.localeCompare(b.id))
  .map(({ sort, ...p }) => p);
writeFileSync("out/registry.json", JSON.stringify({ tag, plugins }, null, 2) + "\n");
console.log(`out/registry.json: ${plugins.length} plugin(s)`);
