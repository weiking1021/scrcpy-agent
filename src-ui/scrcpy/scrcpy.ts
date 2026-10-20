// The release of scrcpy in use: the question before something is downloaded, and the choice of the version in
// the settings. The core decides what is wanted, what is installed and what a change does; this only asks the user
// and shows what the core tells (what GitHub says of a release is asked for when the question is shown, and nothing
// is downloaded before the button is pressed).

import { invoke } from "@tauri-apps/api/core";
import { describeError } from "../shared/errors";
import { t } from "../shared/i18n";
import { $, esc } from "../shared/dom";
import type { Offer, ScrcpyVersions, Snapshot } from "../shared/types";

/** What this part needs of the page around it. */
export interface Host {
  snapshot(): Snapshot | null;
  toast(level: "error" | "info", text: string, detail?: string): void;
  showError(e: unknown): void;
}

let host: Host;

/** The question that is shown: which release, and whether it came from the settings (a choice) or from the start (a need). */
let asked: { tag: string; fromSettings: boolean } | null = null;

/** Releases the user said "not now" to, in this window: they are not asked about again until the window is opened again. */
const declined = new Set<string>();

/** What GitHub told of the releases (asked for with the button of the settings). */
let available: string[] = [];

function size(bytes: number | null): string {
  if (bytes === null) return t("scrcpy.dialog.size_unknown");
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function showOffer(offer: Offer, folder: string): void {
  $("scrcpy-dlg-loading").textContent = "";
  const row = (name: string, value: string, mono = false) =>
    `<dt>${esc(t(name))}</dt><dd${mono ? ' class="mono"' : ""}>${esc(value)}</dd>`;
  const details = $("scrcpy-dlg-details");
  details.innerHTML =
    row("scrcpy.dialog.file", offer.asset) +
    row("scrcpy.dialog.from", offer.url, true) +
    row("scrcpy.dialog.size", size(offer.size)) +
    row("scrcpy.dialog.checked", offer.sha256, true) +
    row("scrcpy.dialog.saved", folder, true);
  details.hidden = false;
  $<HTMLButtonElement>("scrcpy-dlg-ok").disabled = false;
}

/** Opens the question about a release: asks GitHub about it, shows what would be downloaded, and waits for the answer. */
export function askAbout(tag: string, fromSettings = false): void {
  const dlg = $<HTMLDialogElement>("dlg-scrcpy");
  if (dlg.open) return;
  asked = { tag, fromSettings };
  $("scrcpy-dlg-title").textContent = t("scrcpy.dialog.title", { version: tag });
  $("scrcpy-dlg-why").textContent = t("scrcpy.dialog.why");
  $("scrcpy-dlg-loading").textContent = t("scrcpy.dialog.loading");
  $("scrcpy-dlg-details").hidden = true;
  $("scrcpy-dlg-error").textContent = "";
  $<HTMLProgressElement>("scrcpy-dlg-progress").hidden = true;
  const current = host.snapshot()?.scrcpy;
  const keep = $("scrcpy-dlg-keep");
  keep.hidden = !(current?.phase === "ready" && !fromSettings);
  if (current?.phase === "ready") keep.textContent = t("scrcpy.dialog.keep", { version: current.version });
  const ok = $<HTMLButtonElement>("scrcpy-dlg-ok");
  ok.disabled = true;
  ok.textContent = t("scrcpy.download");
  $<HTMLButtonElement>("scrcpy-dlg-later").disabled = false;
  dlg.showModal();
  Promise.all([invoke<Offer>("scrcpy_offer", { tag }), invoke<ScrcpyVersions>("scrcpy_versions")])
    .then(([offer, versions]) => {
      if (asked?.tag === tag) showOffer(offer, versions.folder);
    })
    .catch((e) => {
      $("scrcpy-dlg-loading").textContent = "";
      const { text, detail } = describeError(e);
      $("scrcpy-dlg-error").textContent = detail ? `${text} ${detail}` : text;
    });
}

/** "Not now": a release that was only a choice is dropped; one that was needed is not asked about again in this window, and
 *  if another release is in use meanwhile, that one is kept as the choice (so the question does not come at every start). */
async function later(): Promise<void> {
  const question = asked;
  $<HTMLDialogElement>("dlg-scrcpy").close();
  asked = null;
  if (question === null) return;
  declined.add(question.tag);
  const status = host.snapshot()?.scrcpy;
  if (!question.fromSettings && status?.phase === "ready") {
    try {
      await invoke("scrcpy_use", { tag: status.version });
    } catch (e) {
      host.showError(e);
    }
  }
  await renderChoice();
}

async function download(): Promise<void> {
  const question = asked;
  if (question === null) return;
  const ok = $<HTMLButtonElement>("scrcpy-dlg-ok");
  ok.disabled = true;
  ok.textContent = t("scrcpy.downloading");
  $<HTMLButtonElement>("scrcpy-dlg-later").disabled = true;
  $("scrcpy-dlg-error").textContent = "";
  try {
    await invoke("scrcpy_download", { tag: question.tag });
    $<HTMLDialogElement>("dlg-scrcpy").close();
    asked = null;
    host.toast("info", t("scrcpy.in_use", { version: question.tag }));
  } catch (e) {
    const { text, detail } = describeError(e);
    $("scrcpy-dlg-error").textContent = detail ? `${text} ${detail}` : text;
    ok.disabled = false;
    ok.textContent = t("scrcpy.download");
    $<HTMLButtonElement>("scrcpy-dlg-later").disabled = false;
  }
  await renderChoice();
}

/** The progress of a download that is going on, as the core tells it. */
export function showProgress(progress: number | null): void {
  const bar = $<HTMLProgressElement>("scrcpy-dlg-progress");
  if (progress === null || !$<HTMLDialogElement>("dlg-scrcpy").open) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  bar.value = progress;
}

/** Asks about the release that is wanted, if there is one and the user has not put it off. Called on each state. */
export function checkWanted(): void {
  const wanted = host.snapshot()?.scrcpy_wanted ?? null;
  if (wanted !== null && !declined.has(wanted) && asked === null) askAbout(wanted);
}

// ---------------------------------------------------------------- the choice in the settings

function label(tag: string, versions: ScrcpyVersions): string {
  const notes = [
    tag === versions.default ? t("scrcpy.tag.default") : null,
    tag === versions.current ? t("scrcpy.tag.current") : versions.installed.includes(tag) ? t("scrcpy.tag.installed") : t("scrcpy.tag.not_installed"),
  ].filter((note): note is string => note !== null);
  return `${tag} (${notes.join(", ")})`;
}

/** Newest first: by the numbers of the tag. */
function byVersion(a: string, b: string): number {
  const parts = (tag: string) => tag.slice(1).split(".").map(Number);
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const difference = (y[i] ?? 0) - (x[i] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

/** Puts the releases there are into the list of the settings: what is installed, the default, and what GitHub told. */
export async function renderChoice(): Promise<void> {
  const select = $<HTMLSelectElement>("set-scrcpy-version");
  let versions: ScrcpyVersions;
  try {
    versions = await invoke<ScrcpyVersions>("scrcpy_versions");
  } catch {
    // Not running inside Tauri (plain browser preview).
    return;
  }
  // The release that is wanted is in the list even when it is not installed and GitHub was not asked: the list
  // says what is true (the release in use, else the one that is wanted), whatever was picked and not carried out.
  const tags = [...new Set([versions.default, versions.wanted, ...versions.installed, ...available])].sort(byVersion);
  select.innerHTML = tags.map((tag) => `<option value="${esc(tag)}">${esc(label(tag, versions))}</option>`).join("");
  select.value = versions.current ?? versions.wanted;
  $("set-scrcpy-hint").textContent = t("settings.scrcpy_version_hint");
}

async function choose(): Promise<void> {
  const select = $<HTMLSelectElement>("set-scrcpy-version");
  const tag = select.value;
  let versions: ScrcpyVersions;
  try {
    versions = await invoke<ScrcpyVersions>("scrcpy_versions");
  } catch (e) {
    host.showError(e);
    return;
  }
  if (!versions.installed.includes(tag)) {
    askAbout(tag, true);
    await renderChoice();
    return;
  }
  try {
    await invoke("scrcpy_use", { tag });
    host.toast("info", t("scrcpy.in_use", { version: tag }));
  } catch (e) {
    host.showError(e);
  }
  await renderChoice();
}

async function refresh(): Promise<void> {
  const button = $<HTMLButtonElement>("set-scrcpy-refresh");
  button.disabled = true;
  // The text stays (so the button keeps its size and nothing around it moves); a spinner is drawn over it.
  button.classList.add("busy");
  try {
    available = (await invoke<string[]>("scrcpy_available")) ?? [];
  } catch (e) {
    host.showError(e);
  }
  button.classList.remove("busy");
  button.disabled = false;
  await renderChoice();
}

export function setupScrcpy(next: Host): void {
  host = next;
  $("scrcpy-dlg-later").addEventListener("click", () => void later());
  $("scrcpy-dlg-ok").addEventListener("click", () => void download());
  // Escape closes the dialog as "not now".
  $<HTMLDialogElement>("dlg-scrcpy").addEventListener("cancel", (ev) => {
    ev.preventDefault();
    if (!$<HTMLButtonElement>("scrcpy-dlg-later").disabled) void later();
  });
  $("set-scrcpy-version").addEventListener("change", () => void choose());
  $("set-scrcpy-refresh").addEventListener("click", () => void refresh());
}
