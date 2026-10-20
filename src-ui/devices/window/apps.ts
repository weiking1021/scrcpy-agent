// The apps of the device, in the "apps" tab: the grid of tiles, the search and the kinds (all, favourites, lately
// opened, yours, the system's), opening one, and the favourites. The core keeps the list of apps of the device (it
// shows the kept one at once and reads a new one), the favourites and the apps opened lately; this draws them.

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../../shared/dom";
import { t } from "../../shared/i18n";
import { showError } from "../../shared/toast";
import type { AppCache, AppEntry, AppUsage } from "../../shared/types";
import { openAppMenu } from "./appMenu";
import { moment } from "./format";
import { current, state } from "./state";

let apps: AppEntry[] = [];
let appsUpdatedAt = 0;
/** Increases with every listing, so a slow answer of an earlier one is dropped. */
let appsRequest = 0;

/** Apps that were asked to open and are not yet shown as open; a second click on them is ignored. */
const launchingApps = new Set<string>();

/** How long a tile shows "starting" at least: the window comes up a moment after scrcpy is started. */
const LAUNCH_SPINNER_MS = 2500;

type AppKind = "all" | "favorites" | "recent" | "user" | "system";
const APP_KINDS: readonly string[] = ["all", "favorites", "recent", "user", "system"];
let appKind: AppKind = "all";

/** The favourite apps of this device and the ones opened lately (the core keeps them). */
let usage: AppUsage = { favorites: [], recent: [] };
/** The kind was chosen by the user, so what is found at the start no longer picks it. */
let kindChosen = false;

/** The packages of this device's apps that have a window open now (the core's session keys). */
function openApps(): Set<string> {
  const prefix = `${state.deviceId}#app:`;
  return new Set((state.snap?.sessions ?? []).filter((k) => k.startsWith(prefix)).map((k) => k.slice(prefix.length)));
}

function isAppKind(value: string | undefined): value is AppKind {
  return value !== undefined && APP_KINDS.includes(value);
}

function setKind(kind: AppKind): void {
  appKind = kind;
  $("apps-kind")
    .querySelectorAll<HTMLButtonElement>("button[data-kind]")
    .forEach((x) => x.setAttribute("aria-pressed", String(x.dataset.kind === kind)));
}

/** A tile colour from the package name, so the same app always has the same one. */
function hue(text: string): number {
  let h = 0;
  for (const c of text) h = (h * 31 + c.codePointAt(0)!) % 360;
  return h;
}

/** Draws the tiles for the kind, the search and what is known now. */
export function renderApps(): void {
  const query = $<HTMLInputElement>("apps-search").value.trim().toLowerCase();
  const matches = (a: AppEntry) =>
    query === "" || a.name.toLowerCase().includes(query) || a.package.toLowerCase().includes(query);
  const byPackage = new Map(apps.map((a) => [a.package, a]));
  // The favourites and the recent ones are in the order the core keeps them; an app that is
  // not on the device any more is not shown.
  const listed = (packages: string[]) => packages.flatMap((p) => byPackage.get(p) ?? []);
  let shown: AppEntry[];
  if (appKind === "favorites") shown = listed(usage.favorites).filter(matches);
  else if (appKind === "recent") shown = listed(usage.recent).filter(matches);
  else shown = apps.filter((a) => (appKind === "all" || (appKind === "system") === a.system) && matches(a));
  const favorite = new Set(usage.favorites);
  const open = openApps();
  // scrcpy gives no icons: a tile shows the first letter of the name instead.
  $("apps-list").innerHTML = shown
    .map((a) => {
      const letter = esc(Array.from(a.name.trim())[0]?.toUpperCase() ?? "?");
      const launching = launchingApps.has(a.package);
      const isOpen = open.has(a.package);
      const tileState = launching ? " launching" : isOpen ? " open" : "";
      const busy = launching ? ` aria-busy="true"` : "";
      const marked = favorite.has(a.package);
      const starLabel = esc(t(marked ? "apps.favorite_remove" : "apps.favorite_add"));
      return (
        `<li class="app-cell"><button type="button" class="app-tile${tileState}" data-package="${esc(a.package)}" data-name="${esc(a.name)}"` +
        ` title="${esc(a.package)}"${busy}${launching || isOpen ? " disabled" : ""}>` +
        `<span class="app-letter" style="--hue:${hue(a.package)}" aria-hidden="true">${letter}</span>` +
        `<span class="app-name">${esc(a.name)}</span></button>` +
        `<button type="button" class="app-star${marked ? " marked" : ""}" data-star="${esc(a.package)}" aria-pressed="${marked}"` +
        ` title="${starLabel}" aria-label="${starLabel}">${marked ? "★" : "☆"}</button></li>`
      );
    })
    .join("");
  const line = $("apps-state");
  if (shown.length === 0 && query === "" && appKind === "favorites") {
    line.textContent = t("apps.no_favorites");
  } else if (shown.length === 0 && query === "" && appKind === "recent") {
    line.textContent = t("apps.no_recent");
  } else if (shown.length === 0 && apps.length === 0) {
    line.textContent = t("apps.empty");
  } else if (shown.length === 0) {
    line.textContent = t("apps.no_match");
  } else if (appsUpdatedAt > 0) {
    line.textContent = t("apps.as_of", { when: moment(appsUpdatedAt) });
  } else {
    line.textContent = "";
  }
}

/** Reads the favourites and the apps opened lately, and draws the tiles again. */
export async function loadUsage(): Promise<void> {
  try {
    usage = await invoke<AppUsage>("app_usage", { device: state.deviceId });
  } catch {
    // The device is not known (any more): there is nothing to mark.
    usage = { favorites: [], recent: [] };
  }
  // At the start the recent apps are what one wants most, if there are any.
  if (!kindChosen && usage.recent.length > 0 && appKind === "all") setKind("recent");
  renderApps();
}

/** Shows the kept list at once, then reads a new one and replaces it. */
export async function loadApps(): Promise<void> {
  const request = ++appsRequest;
  try {
    const cache = await invoke<AppCache>("cached_apps", { device: state.deviceId });
    if (request !== appsRequest) return;
    apps = cache.apps;
    appsUpdatedAt = cache.updated_at;
    renderApps();
  } catch (e) {
    showError(e);
  }
  if (current()?.state !== "online") return;
  $("apps-updating").hidden = false;
  try {
    const fresh = await invoke<AppEntry[]>("list_apps", { device: state.deviceId });
    if (request !== appsRequest) return;
    apps = fresh;
    appsUpdatedAt = Math.floor(Date.now() / 1000);
    renderApps();
  } catch (e) {
    if (request !== appsRequest) return;
    // What was kept stays on show; only say that it could not be updated.
    showError(e);
  } finally {
    if (request === appsRequest) $("apps-updating").hidden = true;
  }
}

/** Opens an app; `display` is where the menu asked for, or null for what the device's settings say. */
function openApp(pkg: string, name: string, display: "main_display" | null): void {
  if (launchingApps.has(pkg)) return;
  // Marked before anything else happens, so a quick second click finds it marked.
  launchingApps.add(pkg);
  renderApps();
  const minimum = new Promise((resolve) => setTimeout(resolve, LAUNCH_SPINNER_MS));
  invoke("open_app", { device: state.deviceId, package: pkg, name, display })
    .catch(showError)
    .then(() => minimum)
    .finally(() => {
      launchingApps.delete(pkg);
      // An app that was opened is now the newest of the recent ones.
      void loadUsage();
    });
}

/** Wires the search, the kinds, the tiles, the stars and the menu of a tile. */
export function setupApps(): void {
  $("apps-search").addEventListener("input", renderApps);
  $("apps-kind").addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-kind]");
    const kind = b?.dataset.kind;
    if (!isAppKind(kind)) return;
    kindChosen = true;
    setKind(kind);
    renderApps();
  });
  $("apps-list").addEventListener("click", (ev) => {
    const star = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-star]");
    if (star?.dataset.star) {
      const package_ = star.dataset.star;
      const marked = !usage.favorites.includes(package_);
      invoke("set_app_favorite", { device: state.deviceId, package: package_, favorite: marked })
        .then(loadUsage)
        .catch(showError);
      return;
    }
    const b = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-package]");
    const pkg = b?.dataset.package;
    if (!b || !pkg) return;
    // Where and how it opens is what is set for this device (the "Device settings" tab).
    openApp(pkg, b.dataset.name ?? "", null);
  });
  $("apps-list").addEventListener("contextmenu", (ev) => {
    const cell = (ev.target as HTMLElement).closest<HTMLElement>(".app-cell");
    const tile = cell?.querySelector<HTMLButtonElement>("button[data-package]");
    if (!cell || !tile?.dataset.package) return;
    ev.preventDefault();
    // From the keyboard the event has no position: the menu opens at the tile.
    const box = tile.getBoundingClientRect();
    const fromKeyboard = ev.clientX === 0 && ev.clientY === 0;
    void openAppMenu(
      tile.dataset.package,
      tile.dataset.name ?? "",
      fromKeyboard ? box.left + box.width / 2 : ev.clientX,
      fromKeyboard ? box.top + box.height / 2 : ev.clientY,
      tile,
      { open: openApp, favorites: () => usage.favorites, reloadUsage: loadUsage },
    );
  });
}
