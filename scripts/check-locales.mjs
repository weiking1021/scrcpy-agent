// Verifies the language packs. Run by `npm run build`, or on its own with
// `npm run check:locales`.
//
// - every pack has exactly the keys of the English pack, with string values
// - the {placeholders} of every message match the English message
// - every error code and notice code of the core has a message
//
// The code lists are imported from the UI sources themselves (Node strips
// the type annotations), so they cannot drift apart from this check.

import { readdirSync, readFileSync } from "node:fs";
import { extname, basename } from "node:path";
import {
  ADDON_PERMISSIONS,
  ADDON_STATUSES,
  CORE_ERROR_CODES,
  INSTALL_ERRORS,
  MANIFEST_ERRORS,
  NOTICE_CODES,
  OFFER_ERRORS,
  RELEASE_FAILURES,
  START_FAILURES,
  UNSUPPORTED_REASONS,
} from "../src-ui/shared/types.ts";

const dir = new URL("../src-ui/shared/locales/", import.meta.url);
const packs = Object.fromEntries(
  readdirSync(dir)
    .filter((f) => extname(f) === ".json")
    .map((f) => [basename(f, ".json"), JSON.parse(readFileSync(new URL(f, dir), "utf8"))]),
);

const problems = [];
const en = packs.en;
if (!en) throw new Error("src-ui/shared/locales/en.json is the reference pack and must exist");

const placeholders = (text) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");

for (const [code, pack] of Object.entries(packs)) {
  for (const [key, value] of Object.entries(pack)) {
    if (typeof value !== "string") problems.push(`${code}: "${key}" is not a string`);
  }
  for (const key of Object.keys(en)) {
    if (!(key in pack)) problems.push(`${code}: missing "${key}"`);
    else if (typeof pack[key] === "string" && placeholders(pack[key]) !== placeholders(en[key])) {
      problems.push(`${code}: "${key}" has different {placeholders} than English`);
    }
  }
  for (const key of Object.keys(pack)) {
    if (!(key in en)) problems.push(`${code}: "${key}" does not exist in English`);
  }
  if (typeof pack._name !== "string" || pack._name === "") problems.push(`${code}: "_name" is required`);
}

for (const code of CORE_ERROR_CODES) {
  if (!(`error.${code}` in en)) problems.push(`en: no message for error code "${code}" (error.${code})`);
}
for (const code of NOTICE_CODES) {
  if (!(`notice.${code}` in en)) problems.push(`en: no message for notice code "${code}" (notice.${code})`);
}

// Every reason an add-on can be refused for, every status it can have and every permission has a message.
const addonKeys = [
  ...INSTALL_ERRORS.map((r) => `addon.error.${r}`),
  ...ADDON_STATUSES.map((s) => `addon.status.${s}`),
  ...ADDON_PERMISSIONS.flatMap((p) => [`addon.permission.${p}`, `addon.permission_short.${p}`]),
  ...MANIFEST_ERRORS.flatMap((r) => [`addon.error.manifest.${r}`, `addon.status.manifest.${r}`]),
  ...UNSUPPORTED_REASONS.flatMap((r) => [`addon.error.unsupported.${r}`, `addon.status.unsupported.${r}`]),
];
addonKeys.push(
  ...RELEASE_FAILURES.map((f) => `scrcpy.error.${f}`),
  ...OFFER_ERRORS.map((f) => `scrcpy.error.offer.${f}`),
);
addonKeys.push("addon.run.running", ...START_FAILURES.map((f) => `addon.run.failed.${f}`));
for (const key of addonKeys) {
  if (!(key in en)) problems.push(`en: no message "${key}"`);
}

if (problems.length > 0) {
  console.error(problems.map((p) => `  - ${p}`).join("\n"));
  console.error(`\n${problems.length} language pack problem(s).`);
  process.exit(1);
}
console.log(`Language packs OK (${Object.keys(packs).join(", ")}; ${Object.keys(en).length} messages).`);
