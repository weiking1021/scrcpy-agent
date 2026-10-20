// The list of the buttons of the floating toolbar in the settings: a tick shows or hides a button, and the arrows
// move it. The core says which buttons there are and in what order (it knows the buttons of the add-ons too); this
// only changes the list, and what it holds is kept with the other settings.

import { invoke } from "@tauri-apps/api/core";
import { buttonIcon, buttonName } from "../shared/buttons";
import { $, esc } from "../shared/dom";
import { t } from "../shared/i18n";
import type { ButtonPref, LayoutEntry } from "../shared/types";

/** What this part needs of the page around it. */
export interface LayoutHost {
  /** The list was changed: keep it. */
  changed(): void;
  /** The user wants the default order and every button shown: keep that. */
  reset(): void;
}

let layout: LayoutEntry[] = [];

/** The layout as the settings keep it. */
export function layoutPrefs(): ButtonPref[] {
  return layout.map((entry) => ({ id: entry.id, visible: entry.visible }));
}

function draw(): void {
  $("set-toolbar-buttons").innerHTML = layout
    .map((entry, index) => {
      const name = esc(buttonName(entry.id, entry.label));
      const up = esc(t("settings.toolbar_button_up"));
      const down = esc(t("settings.toolbar_button_down"));
      return (
        `<li data-index="${index}">` +
        `<label class="check"><input type="checkbox" data-visible${entry.visible ? " checked" : ""} />` +
        `<span class="layout-icon">${buttonIcon(entry.id)}</span><span>${name}</span></label>` +
        `<span class="layout-move">` +
        `<button type="button" data-move="-1" title="${up}" aria-label="${up}"${index === 0 ? " disabled" : ""}>↑</button>` +
        `<button type="button" data-move="1" title="${down}" aria-label="${down}"${index === layout.length - 1 ? " disabled" : ""}>↓</button>` +
        `</span></li>`
      );
    })
    .join("");
}

/** Asks the core for the buttons there are now (an add-on may have added or taken one) and draws them. */
export async function loadToolbarLayout(): Promise<void> {
  try {
    layout = await invoke<LayoutEntry[]>("toolbar_layout");
  } catch {
    // Not running inside Tauri (plain browser preview).
    layout = [];
  }
  draw();
}

export function setupToolbarLayout(host: LayoutHost): void {
  const list = $("set-toolbar-buttons");
  // The tick is read here, before the page keeps its settings (a change reaches this list first).
  list.addEventListener("change", (ev) => {
    const box = ev.target as HTMLInputElement;
    const index = Number(box.closest<HTMLElement>("li")?.dataset.index);
    if (box.matches("input[data-visible]") && layout[index]) layout[index].visible = box.checked;
  });
  list.addEventListener("click", (ev) => {
    const button = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-move]");
    const index = Number(button?.closest<HTMLElement>("li")?.dataset.index);
    const to = index + Number(button?.dataset.move);
    if (!button || !layout[index] || !layout[to]) return;
    [layout[index], layout[to]] = [layout[to], layout[index]];
    draw();
    host.changed();
  });
  $("set-toolbar-reset").addEventListener("click", () => host.reset());
}
