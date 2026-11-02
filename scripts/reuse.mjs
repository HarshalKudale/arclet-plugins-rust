// Carries every plugin that was not rebuilt over from the previous release: downloads all of its
// <dir>_<os>_* assets into out/ so the new release is complete.
//   node scripts/reuse.mjs <previous tag> <dir with its manifests>
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";

const [tag, prevDir] = process.argv.slice(2);
for (const f of readdirSync(prevDir).filter((f) => f.endsWith("_manifest.json"))) {
  if (existsSync(`out/${f}`)) continue; // rebuilt in this run
  const prefix = f.replace(/manifest\.json$/, "");
  execFileSync("gh", ["release", "download", tag, "-p", `${prefix}*`, "-D", "out"], { stdio: "inherit" });
  console.log(`reused ${prefix}*`);
}
