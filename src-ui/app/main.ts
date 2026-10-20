// The main window: starts the parts of the page (the devices, the settings, the add-ons, the release of scrcpy, the
// files), keeps the latest state the core announced, and draws the page again when it changes. What each part does
// is in its own folder; this only joins them.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { refreshAddons, setupAddons } from "../addons/addons";
import { loadInfos, renderDeviceList, setupDeviceList, showInfo } from "../devices/list";
import { renderPairFound, setupPairDialog } from "../devices/pairing";
import { initFiles, onOpened, onTransfer } from "../files/files";
import { renderScrcpyStatus } from "../scrcpy/phase";
import { checkWanted, renderChoice, setupScrcpy } from "../scrcpy/scrcpy";
import { loadSystemFeatures, setupSettingsPage } from "../settings/page";
import { suppressNativeContextMenu } from "../shared/contextmenu";
import { loadUserPacks, setLanguage, translateDom } from "../shared/i18n";
import { initRipple } from "../shared/ripple";
import { initTheme } from "../shared/theme";
import { showError, toast } from "../shared/toast";
import type { CoreEvent, Snapshot } from "../shared/types";
import { applyIdentity } from "./identity";
import { setupNavigation } from "./navigation";
import { showNotice } from "./notices";

/** The latest state the core announced; `null` until the first one. */
let snap: Snapshot | null = null;
const snapshot = (): Snapshot | null => snap;

/** Draws the page for the latest state. */
function render(): void {
  if (!snap) return;
  renderScrcpyStatus(snap);
  renderDeviceList(snap);
  renderPairFound();
}

/** The state the core announced: the page follows it, and the language or the name may have changed with it. */
function onState(next: Snapshot): void {
  const first = snap === null;
  const identityChanged =
    snap?.settings.language !== next.settings.language || snap?.settings.app_name !== next.settings.app_name;
  snap = next;
  if (first || identityChanged) applyIdentity(snap.settings);
  render();
  checkWanted();
}

async function main(): Promise<void> {
  initRipple();
  void initTheme();
  await loadUserPacks();
  setLanguage(null);
  translateDom();
  setupPairDialog({ snapshot });
  setupSettingsPage({
    snapshot,
    saved: (settings) => {
      // What was saved is what the page shows from now on, without waiting for the state event.
      if (snap) snap = { ...snap, settings };
      applyIdentity(settings);
      render();
    },
    filled: () => {
      void renderChoice();
      // Each folder is read again (one may have been changed or be gone).
      void refreshAddons();
    },
  });
  setupAddons({ toast, showError });
  setupScrcpy({ snapshot, toast, showError });
  suppressNativeContextMenu();
  setupNavigation();
  setupDeviceList({ snapshot, render });

  await listen<CoreEvent>("core-event", (e) => {
    const ev = e.payload;
    if (ev.kind === "state") {
      onState(ev.data);
    } else if (ev.kind === "notice") {
      showNotice(ev.data);
    } else if (ev.kind === "device_info") {
      showInfo(ev.data.device, ev.data.info);
    } else if (ev.kind === "transfer") {
      onTransfer(ev.data);
    } else if (ev.kind === "opened") {
      onOpened(ev.data);
    }
    // (log lines are for the debug console window, and the requests of a file manager for the window of a request)
  });

  // The first state event may have been sent before we started listening.
  snap = await invoke<Snapshot>("get_snapshot");
  applyIdentity(snap.settings);
  checkWanted();
  await loadSystemFeatures();
  // Readings made before this window was listening.
  await loadInfos();
  render();
  await initFiles({ snapshot, toast, showError });
}

main().catch((e) => console.error(e));
