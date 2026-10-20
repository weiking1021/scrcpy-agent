// The buttons of the floating toolbar: how each is drawn and what it is called. The toolbar and the list in the
// settings (where the user arranges them) show them the same way.

import { t } from "./i18n";
import type { BuiltInButton, ButtonId } from "./types";

/** Icons are drawn with strokes on a 24 x 24 grid. */
const ICONS: Record<BuiltInButton | "addon", string> = {
  back: '<path d="M15 5l-7 7 7 7"/>',
  home: '<circle cx="12" cy="12" r="7"/>',
  app_switch: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  power: '<path d="M12 4v8"/><path d="M7.5 7a7 7 0 1 0 9 0"/>',
  volume_up: '<path d="M4 10h3l4-3v10l-4-3H4z"/><path d="M15 12h6M18 9v6"/>',
  volume_down: '<path d="M4 10h3l4-3v10l-4-3H4z"/><path d="M15 12h6"/>',
  rotate: '<path d="M18.5 8A7 7 0 1 0 19 13"/><path d="M18.5 4v4h-4"/>',
  notifications: '<path d="M7 17h10l-1.5-2v-4a3.5 3.5 0 0 0-7 0v4z"/><path d="M10.5 20h3"/>',
  send_file: '<path d="M12 16V6"/><path d="M8 10l4-4 4 4"/><path d="M6 19h12"/>',
  screenshot: '<path d="M5 9V5h4M15 5h4v4M19 15v4h-4M9 19H5v-4"/><circle cx="12" cy="12" r="2"/>',
  addon: '<circle cx="12" cy="12" r="2"/><path d="M12 5v3M12 16v3M5 12h3M16 12h3"/>',
};

/** Every built-in button has a name in each language pack. */
const LABELS: Record<BuiltInButton, string> = {
  back: "toolbar.button.back",
  home: "toolbar.button.home",
  app_switch: "toolbar.button.app_switch",
  power: "toolbar.button.power",
  volume_up: "toolbar.button.volume_up",
  volume_down: "toolbar.button.volume_down",
  rotate: "toolbar.button.rotate",
  notifications: "toolbar.button.notifications",
  send_file: "toolbar.button.send_file",
  screenshot: "toolbar.button.screenshot",
};

/** The icon of a button, as an `<svg>` (a button of an add-on has a plain one). */
export function buttonIcon(id: ButtonId): string {
  return `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[typeof id === "string" ? id : "addon"]}</svg>`;
}

/** What a button is called: the language pack's name for a built-in one, the text the add-on gave its own. */
export function buttonName(id: ButtonId, label: string | null): string {
  return typeof id === "string" ? t(LABELS[id]) : (label ?? id.addon.id);
}
