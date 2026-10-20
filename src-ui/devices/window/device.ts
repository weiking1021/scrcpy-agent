// The window of one device: what is known about it, a name of the user's own, its apps and what is set for it. It
// is a page of its own (one window per device); the id of the device is in the window's label. This starts the parts
// of the window (in this folder) and follows the state the core announces.

import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { suppressNativeContextMenu } from "../../shared/contextmenu";
import { $ } from "../../shared/dom";
import { loadUserPacks, setAppName, setLanguage, translateDom } from "../../shared/i18n";
import { initRipple } from "../../shared/ripple";
import { initTheme } from "../../shared/theme";
import type { CoreEvent, Snapshot } from "../../shared/types";
import { loadApps, loadUsage, renderApps, setupApps } from "./apps";
import { renderAddons, renderHeader, setupHeader } from "./header";
import { loadDetails, renderInfo } from "./info";
import { loadDeviceSettings, wireDeviceSettings } from "./settings";
import { current, state } from "./state";

const LABEL_PREFIX = "device-";

/** The device id inside the window label (the label is the id in hexadecimal). */
function deviceIdFromLabel(label: string): string | null {
  if (!label.startsWith(LABEL_PREFIX)) return null;
  const hex = label.slice(LABEL_PREFIX.length);
  if (hex.length === 0 || hex.length % 2 !== 0 || !/^[0-9a-f]+$/.test(hex)) return null;
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

const TABS = ["apps", "info", "settings"] as const;

/** Shows one of the three parts of the window: the apps, what is known, or what is set for the device. */
function showTab(name: string): void {
  const shown = (TABS as readonly string[]).includes(name) ? name : "apps";
  for (const tab of TABS) {
    $(`tab-${tab}`).hidden = tab !== shown;
    $(`tab-btn-${tab}`).setAttribute("aria-selected", String(tab === shown));
  }
  if (shown === "settings") void loadDeviceSettings();
}

function wire(): void {
  suppressNativeContextMenu();
  setupHeader(() => void loadApps());
  setupApps();
  document.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach((b) => {
    b.addEventListener("click", () => showTab(b.dataset.tab ?? "apps"));
  });
  wireDeviceSettings();
}

/** The state the core announced: the header, the apps that are open, and what changed about the device. */
function onState(next: Snapshot): void {
  const before = current();
  const language = state.snap?.settings.language;
  state.snap = next;
  setLanguage(next.settings.language);
  setAppName(next.settings.app_name);
  translateDom();
  const after = current();
  renderHeader();
  // Which apps have a window open may have changed.
  renderApps();
  if (language !== next.settings.language) renderInfo();
  // The device came online or went away: read it again.
  if (before?.state !== after?.state) {
    void renderAddons();
    void loadDetails();
    if (after?.state === "online") void loadApps();
  } else if (before?.label !== after?.label && state.details) {
    state.details = { ...state.details, device: after ?? state.details.device };
    renderInfo();
  }
}

async function main(): Promise<void> {
  initRipple();
  void initTheme();
  const id = deviceIdFromLabel(getCurrentWindow().label);
  if (id === null) {
    document.body.textContent = "?";
    return;
  }
  state.deviceId = id;
  await loadUserPacks();
  wire();

  await listen<CoreEvent>("core-event", (e) => {
    const ev = e.payload;
    if (ev.kind === "device_info") {
      // A new reading of this device (the core reads them all on its own).
      if (ev.data.device === state.deviceId && state.details) {
        state.details = { ...state.details, info: ev.data.info };
        renderInfo();
      }
      return;
    }
    if (ev.kind === "state") onState(ev.data);
  });

  state.snap = await invoke<Snapshot>("get_snapshot");
  setLanguage(state.snap.settings.language);
  setAppName(state.snap.settings.app_name);
  translateDom();
  renderHeader();
  await renderAddons();
  void loadDetails();
  void loadApps();
  void loadUsage();
}

main().catch((e) => console.error(e));
