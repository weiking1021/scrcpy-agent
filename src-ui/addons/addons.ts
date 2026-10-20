// The add-ons in the settings: the list of the installed ones, adding one from a folder, and the
// question of which permissions it gets. The core decides everything (what an add-on is, whether it can
// be installed, what it may do); this is only what the user sees of it and answers.

import { invoke } from "@tauri-apps/api/core";
import { addonReasonKey } from "../shared/errors";
import { t } from "../shared/i18n";
import { $, esc } from "../shared/dom";
import { openAddonSettings, setupAddonSettings } from "./settings";
import { ADDON_PERMISSIONS } from "../shared/types";
import type { AddonManifest, AddonPermission, InstalledAddon, StartFailure } from "../shared/types";

/** What this part needs of the page around it. */
export interface Host {
  toast(level: "error" | "info", text: string, detail?: string): void;
  showError(e: unknown): void;
}

let host: Host;
let installed: InstalledAddon[] = [];

/** Which permission names the user sees as a warning. */
const DANGEROUS: readonly AddonPermission[] = ["run_process", "screen_capture"];

/** What the dialog is about: a folder to install from, or an installed add-on to decide again about. */
type Question = { kind: "install"; folder: string; manifest: AddonManifest } | { kind: "decide"; manifest: AddonManifest };

let question: Question | null = null;

function permissionName(p: AddonPermission): string {
  return t(`addon.permission.${p}`);
}

/** What the add-on is doing, as a line: that it runs, or why it did not start (the text of the script is shown under it). */
function runLine(a: InstalledAddon): string {
  if (a.run === "disabled") return `<p class="setting-hint">${esc(t("addon.run.disabled"))}</p>`;
  if (a.status !== "ready" || a.run === "not_running") return "";
  if (a.run === "running") return `<p class="setting-hint ok">${esc(t("addon.run.running"))}</p>`;
  const failure: StartFailure = a.run.failed;
  if (typeof failure === "string") return `<p class="setting-hint bad">${esc(t(`addon.run.failed.${failure}`))}</p>`;
  const kind = "script" in failure ? "script" : "thread";
  const text = "script" in failure ? failure.script : failure.thread;
  return `<p class="setting-hint bad">${esc(t(`addon.run.failed.${kind}`))}</p><pre class="addon-error">${esc(text)}</pre>`;
}

/** What it tried to use without the permission, if anything. */
function deniedLine(a: InstalledAddon): string {
  if (a.denied.length === 0) return "";
  const list = a.denied.map((p) => t(`addon.permission_short.${p}`)).join(", ");
  return `<p class="setting-hint bad">${esc(t("addon.denied", { list }))}</p>`;
}

function renderList(): void {
  const list = $("addon-list");
  $("addon-none").hidden = installed.length > 0;
  list.innerHTML = installed
    .map((a) => {
      const manifest = a.manifest;
      const title = manifest ? `${manifest.name}` : a.id;
      const meta = manifest ? t("addon.meta", { id: a.id, version: manifest.version }) : a.id;
      const ready = a.status === "ready";
      const status = ready ? "" : `<p class="setting-hint bad">${esc(t(addonReasonKey("addon.status", a.status)))}</p>`;
      const allowed = a.granted.length
        ? t("addon.allowed", { list: a.granted.map((p) => t(`addon.permission_short.${p}`)).join(", ") })
        : t("addon.allowed_none");
      const asks = manifest !== null && manifest.permissions.length > 0;
      return (
        `<div class="setting addon-row" data-addon="${esc(a.id)}">` +
        `<div class="setting-text"><span class="setting-name">${esc(title)}</span>` +
        `<p class="setting-hint mono">${esc(meta)}</p>${status}${runLine(a)}${deniedLine(a)}` +
        (asks ? `<p class="setting-hint">${esc(allowed)}</p>` : "") +
        `</div><div class="setting-control addon-actions">` +
        (a.enabled ? `<button type="button" data-act="reload">${esc(t("addon.reload"))}</button>` : "") +
        `<button type="button" data-act="toggle">${esc(t(a.enabled ? "addon.disable" : "addon.enable"))}</button>` +
        (manifest !== null && manifest.settings.length > 0
          ? `<button type="button" data-act="settings">${esc(t("addon.settings"))}</button>`
          : "") +
        (asks ? `<button type="button" data-act="decide">${esc(t("addon.decide"))}</button>` : "") +
        `<button type="button" data-act="remove">${esc(t("addon.remove"))}</button>` +
        `</div></div>`
      );
    })
    .join("");
}

/** Reads the list again from the core (each folder is read again there, so what happened to it shows). */
export async function refreshAddons(): Promise<void> {
  try {
    installed = await invoke<InstalledAddon[]>("list_addons");
  } catch {
    // Not running inside Tauri (plain browser preview).
    installed = [];
  }
  renderList();
}

function openQuestion(next: Question, given: readonly AddonPermission[]): void {
  question = next;
  const manifest = next.manifest;
  $("addon-dlg-title").textContent = t(next.kind === "install" ? "addon.title_install" : "addon.title_decide", {
    name: manifest.name,
  });
  $("addon-dlg-meta").textContent = t("addon.meta", { id: manifest.id, version: manifest.version });
  $("addon-dlg-asks").textContent = t(manifest.permissions.length ? "addon.asks_intro" : "addon.asks_nothing");
  // The permissions of the add-on, in the order of its manifest (any that the interface does not know cannot be here:
  // the core refuses such a manifest).
  $("addon-dlg-list").innerHTML = ADDON_PERMISSIONS.filter((p) => manifest.permissions.includes(p))
    .map(
      (p) =>
        `<label class="check${DANGEROUS.includes(p) ? " warn" : ""}"><input type="checkbox" data-permission="${p}"` +
        `${given.includes(p) ? " checked" : ""} /><span>${esc(permissionName(p))}</span></label>`,
    )
    .join("");
  $("addon-dlg-ok").textContent = t(next.kind === "install" ? "addon.install" : "addon.save");
  const dlg = $<HTMLDialogElement>("dlg-addon");
  if (!dlg.open) dlg.showModal();
}

/** The permissions ticked in the dialog. */
function ticked(): AddonPermission[] {
  return [...$("addon-dlg-list").querySelectorAll<HTMLInputElement>("input[data-permission]")]
    .filter((box) => box.checked)
    .flatMap((box) => ADDON_PERMISSIONS.filter((p) => p === box.dataset.permission));
}

async function answer(): Promise<void> {
  const asked = question;
  if (asked === null) return;
  const granted = ticked();
  try {
    if (asked.kind === "install") {
      await invoke("install_addon", { folder: asked.folder, granted });
      host.toast("info", t("addon.installed", { name: asked.manifest.name }));
    } else {
      await invoke("decide_addon_permissions", { id: asked.manifest.id, granted });
    }
    $<HTMLDialogElement>("dlg-addon").close();
    question = null;
  } catch (e) {
    $<HTMLDialogElement>("dlg-addon").close();
    host.showError(e);
  }
  await refreshAddons();
}

/** Asks for a folder with the system's dialog, then looks at it: what it is and what it asks for, then the question. */
async function addFromFolder(): Promise<void> {
  try {
    const folder = await invoke<string | null>("choose_addon_folder", { title: t("addon.pick_folder") });
    if (folder === null) return;
    const manifest = await invoke<AddonManifest>("inspect_addon_folder", { folder });
    // Nothing is given until the user ticks it: a permission is a decision, not a default.
    openQuestion({ kind: "install", folder, manifest }, []);
  } catch (e) {
    host.showError(e);
  }
}

export function setupAddons(next: Host): void {
  host = next;
  const source = $<HTMLDialogElement>("dlg-addon-source");
  $("addon-add").addEventListener("click", () => source.showModal());
  $("addon-source-cancel").addEventListener("click", () => source.close());
  $("addon-source-folder").addEventListener("click", () => {
    source.close();
    void addFromFolder();
  });
  setupAddonSettings();
  $("addon-dlg-cancel").addEventListener("click", () => $<HTMLDialogElement>("dlg-addon").close());
  $("addon-dlg-ok").addEventListener("click", () => void answer());
  $("addon-list").addEventListener("click", (ev) => {
    const button = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-act]");
    const id = button?.closest<HTMLElement>("[data-addon]")?.dataset.addon;
    const addon = installed.find((a) => a.id === id);
    if (!button || !addon) return;
    if (button.dataset.act === "remove") {
      invoke<boolean>("uninstall_addon", { id: addon.id })
        .then(() => host.toast("info", t("addon.removed", { id: addon.id })))
        .catch(host.showError)
        .finally(() => void refreshAddons());
    } else if (button.dataset.act === "toggle") {
      button.disabled = true;
      invoke("set_addon_enabled", { id: addon.id, enabled: !addon.enabled })
        .catch(host.showError)
        .finally(() => void refreshAddons());
    } else if (button.dataset.act === "reload") {
      button.disabled = true;
      invoke("reload_addon", { id: addon.id })
        .catch(host.showError)
        .finally(() => void refreshAddons());
    } else if (button.dataset.act === "settings") {
      void openAddonSettings(addon.id, addon.manifest?.name ?? addon.id);
    } else if (addon.manifest) {
      openQuestion({ kind: "decide", manifest: addon.manifest }, addon.granted);
    }
  });
}
