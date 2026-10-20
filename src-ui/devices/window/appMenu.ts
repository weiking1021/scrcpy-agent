// The menu of an app (the right button on a tile): the core says which entries there are and which can be used, and
// the entries of the add-ons come after the built-in ones. What the entries do to the list of apps is given by
// `AppActions`, so this does not need to know how the list is kept.

import { invoke } from "@tauri-apps/api/core";
import { type PopupEntry, showPopupMenu } from "../../shared/contextmenu";
import { t } from "../../shared/i18n";
import { showError, toast } from "../../shared/toast";
import type { AppMenuItem, ShortcutKind, ShortcutPlace, ShortcutState } from "../../shared/types";
import { state } from "./state";

/** What the entries of the menu do to the list of apps. */
export interface AppActions {
  /** Opens the app; `display` is where the menu asked for, or null for what the device's settings say. */
  open(pkg: string, name: string, display: "main_display" | null): void;
  /** The packages marked as favourites now. */
  favorites(): string[];
  /** Reads what is marked and what was opened lately again. */
  reloadUsage(): Promise<void>;
}

export async function openAppMenu(
  pkg: string,
  name: string,
  x: number,
  y: number,
  tile: HTMLElement,
  actions: AppActions,
): Promise<void> {
  const device = state.deviceId;
  let items: AppMenuItem[];
  let shortcuts: ShortcutState | null = null;
  // What the menu of applications is called depends on the system.
  let menuWords = { add: "apps.menu_shortcut_start", remove: "apps.menu_shortcut_start_remove" };
  try {
    items = await invoke<AppMenuItem[]>("app_menu", { device, package: pkg });
    // Where shortcuts cannot be made the state is refused: the entries are left out.
    const kind = await invoke<ShortcutKind | null>("shortcuts_available");
    if (kind !== null) {
      shortcuts = await invoke<ShortcutState>("app_shortcut_state", { device, package: pkg });
      if (kind === "desktop_entry") {
        menuWords = { add: "apps.menu_shortcut_menu", remove: "apps.menu_shortcut_menu_remove" };
      }
    }
  } catch (e) {
    showError(e);
    return;
  }
  const marked = actions.favorites().includes(pkg);
  const makeShortcut = (place: ShortcutPlace) =>
    invoke<string>("create_app_shortcut", { device, package: pkg, place })
      .then((path) => toast("info", t("apps.shortcut_made"), path))
      .catch(showError);
  const removeShortcut = (place: ShortcutPlace) =>
    invoke("remove_app_shortcut", { device, package: pkg, place })
      .then(() => toast("info", t("apps.shortcut_removed")))
      .catch(showError);
  const entries: PopupEntry[] = [];
  let addons = false;
  for (const item of items) {
    const id = item.id;
    if (typeof id === "object") {
      entries.push({
        label: item.label ?? id.addon.id,
        enabled: item.enabled,
        separator: !addons,
        run: () => void invoke("run_app_menu_item", { device, package: pkg, item: id }).catch(showError),
      });
      addons = true;
      continue;
    }
    switch (id) {
      case "open":
        entries.push({ label: t("apps.menu_open"), enabled: item.enabled, run: () => actions.open(pkg, name, null) });
        break;
      case "open_on_main_screen":
        entries.push({ label: t("apps.menu_open_main"), enabled: item.enabled, run: () => actions.open(pkg, name, "main_display") });
        break;
      case "toggle_favorite":
        entries.push({
          label: t(marked ? "apps.favorite_remove" : "apps.favorite_add"),
          enabled: item.enabled,
          run: () =>
            void invoke("set_app_favorite", { device, package: pkg, favorite: !marked })
              .then(actions.reloadUsage)
              .catch(showError),
        });
        break;
      case "shortcut_desktop":
        if (shortcuts) {
          const there = shortcuts.desktop;
          entries.push({
            label: t(there ? "apps.menu_shortcut_desktop_remove" : "apps.menu_shortcut_desktop"),
            enabled: there || item.enabled,
            separator: true,
            run: () => void (there ? removeShortcut("desktop") : makeShortcut("desktop")),
          });
        }
        break;
      case "shortcut_start_menu":
        if (shortcuts) {
          const there = shortcuts.start_menu;
          entries.push({
            label: t(there ? menuWords.remove : menuWords.add),
            enabled: there || item.enabled,
            run: () => void (there ? removeShortcut("start_menu") : makeShortcut("start_menu")),
          });
        }
        break;
    }
  }
  showPopupMenu(x, y, entries, tile);
}
