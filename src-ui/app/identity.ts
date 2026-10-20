// What the program is called and the language it speaks: the settings choose them, and the window, its title, the
// tray menu (built by the core, which has no language packs) and the menu of the file manager all say them.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { setAppName, setLanguage, t } from "../shared/i18n";
import type { Settings } from "../shared/types";
import { translateAll } from "./navigation";

/** The tray menu is built by the core, which has no language packs: it gets its texts from here. */
function sendTrayLabels(): void {
  const texts = {
    app_name: t("app.title"),
    show: t("tray.show"),
    quit: t("tray.quit"),
    open_screen: t("tray.open_screen"),
    devices: t("tray.devices"),
    none: t("tray.none"),
    state_online: t("state.online"),
    state_unauthorized: t("state.unauthorized"),
    state_offline: t("state.offline"),
    state_discovered: t("state.discovered"),
    state_disconnected: t("state.disconnected"),
  };
  invoke("set_tray_texts", { texts }).catch(() => {});
  // The entries of the Explorer menu are in the same language (written again if the menu is on).
  invoke("set_explorer_menu_texts", {
    texts: {
      title: t("app.title"),
      send_default: t("explorer.send_default"),
      send_choose: t("explorer.send_choose"),
      open_default: t("explorer.open_default"),
      open_choose: t("explorer.open_choose"),
    },
  }).catch(() => {});
}

/**
 * Makes the language and the name of the program from the settings what the window, the tray and the
 * menu of the file manager say. The texts are written again, so it is only called when one of them changed.
 */
export function applyIdentity(settings: Settings): void {
  setLanguage(settings.language);
  setAppName(settings.app_name);
  translateAll();
  getCurrentWindow().setTitle(t("app.title")).catch(() => {});
  sendTrayLabels();
}
