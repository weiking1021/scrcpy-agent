// Builds every package into `out/`, with Docker: the Linux program and the .deb
// (one build), and the Windows installer and portable zip; and the types package of the add-ons
// (`addon-api/`, a .tgz made by `npm pack`). Run with `npm run package`.
//
// The two builds run at the same time by default. On machines with less memory,
// `npm run package -- --sequential` runs them one after the other. `out/` ends
// with only what this run built: each build exports into a folder of its own, and `out/`
// is replaced only when both succeeded, so a failed build leaves the previous packages as
// they were. The exit code is not 0 when any build failed (a CI job then fails).

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readdirSync, renameSync, rmSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";

const OUT = "out";
const sequential = process.argv.includes("--sequential");
// The targets of the Dockerfile that export files, each with the folder it exports into
// and the name its lines are shown with.
const BUILDS = [
  { target: "export-linux", folder: "out.tmp-linux", name: "linux  " },
  { target: "export-windows", folder: "out.tmp-windows", name: "windows" },
];

/** Runs one build and resolves with its exit code (a program that cannot be started is 1). */
function build({ target, folder, name }) {
  return new Promise((resolve) => {
    const args = ["build", "--progress=plain", "--target", target, "--output", `type=local,dest=${folder}`, "."];
    const child = spawn("docker", args, { stdio: ["ignore", "pipe", "pipe"] });
    for (const stream of [child.stdout, child.stderr]) {
      createInterface({ input: stream }).on("line", (line) => console.log(`[${name}] ${line}`));
    }
    child.on("error", (error) => {
      console.error(`[${name}] docker could not be started: ${error.message}`);
      resolve(1);
    });
    child.on("close", (code) => resolve(code ?? 1));
  });
}

for (const { folder } of BUILDS) rmSync(folder, { recursive: true, force: true });
const codes = [];
if (sequential) {
  for (const target of BUILDS) codes.push(await build(target));
} else {
  codes.push(...await Promise.all(BUILDS.map(build)));
}
const failed = BUILDS.filter((_, index) => codes[index] !== 0);

if (failed.length > 0) {
  for (const { folder } of BUILDS) rmSync(folder, { recursive: true, force: true });
  console.error(`\nThe build failed (${failed.map((b) => b.target).join(", ")}); ${OUT}/ was not touched.`);
  process.exit(codes.find((code) => code !== 0) ?? 1);
}

// The folder itself stays: a file manager or an editor that has it open would make
// Windows refuse to delete it. Only what is in it is replaced.
mkdirSync(OUT, { recursive: true });
for (const entry of readdirSync(OUT)) rmSync(join(OUT, entry), { recursive: true, force: true });
for (const { folder } of BUILDS) {
  for (const entry of readdirSync(folder)) renameSync(join(folder, entry), join(OUT, entry));
  rmSync(folder, { recursive: true, force: true });
}

// The types for the authors of add-ons go with the release (they are not published to npm).
try {
  execFileSync("npm", ["pack", "./addon-api", "--pack-destination", OUT, "--silent"], {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
} catch {
  console.error(`\nThe types package of the add-ons could not be made; the other packages are in ${OUT}/.`);
  process.exit(1);
}
console.log(`\nDone: the packages are in ${OUT}/`);
