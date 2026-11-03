// Drive a built plugin over the real stdio protocol, as the host does (CLAUDE.md Rule 3).
//
//   node scripts/drive.mjs <plugin exe> <steps.json>
//
// steps.json is an array run in order after hello → ready. Each step is either a raw protocol
// message ({"type": "sync" | "invoke" | "request" | "event", ...}) or {"wait": ms}. Requests get an
// id when they have none. Every line the plugin prints is echoed; a non-JSON stdout line fails.
// KD_DATA_DIR sets the plugin's data folder (to pre-place .env or a sign-in token).
import { spawn } from "node:child_process";
import { readFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

const [exe, stepsFile] = process.argv.slice(2);
if (!exe || !stepsFile) {
  console.error("usage: node scripts/drive.mjs <plugin exe> <steps.json>");
  process.exit(2);
}
const steps = JSON.parse(readFileSync(stepsFile, "utf8"));
const child = spawn(exe, [], { stdio: ["pipe", "pipe", "inherit"] });
let failed = false;
let ready;
const isReady = new Promise((r) => (ready = r));

createInterface({ input: child.stdout }).on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    console.log(`!! non-protocol stdout: ${line}`);
    failed = true;
    return;
  }
  if (msg.type === "ready") ready();
  if (msg.type === "error" || msg.ok === false) failed = true;
  console.log(`<- ${line}`);
});

const send = (msg) => {
  console.log(`-> ${JSON.stringify(msg)}`);
  child.stdin.write(JSON.stringify(msg) + "\n");
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

send({ type: "hello", protocol: 2, plugin_id: "drive", data_dir: process.env.KD_DATA_DIR ?? mkdtempSync(join(tmpdir(), "kd-drive-")), host_version: "drive" });
await Promise.race([isReady, sleep(10000).then(() => Promise.reject(new Error("no ready within 10 s")))]);
let n = 0;
for (const step of steps) {
  if (step.wait) await sleep(step.wait);
  else {
    if (step.type === "request" || step.type === "invoke") step.id ??= `s${++n}`;
    send(step);
    await sleep(300);
  }
}
await sleep(1000);
send({ type: "shutdown" });
child.on("exit", (code) => {
  console.log(failed ? "FAILED (an error or failed result was reported)" : `done (exit ${code})`);
  process.exit(failed ? 1 : 0);
});
