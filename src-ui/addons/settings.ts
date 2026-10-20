// The settings of one add-on: a dialog with a form made from what the add-on declared in its manifest (a switch, a
// whole number, a line of text, a choice). A change is kept as soon as it is made; the core checks it against what was
// declared (a value it refuses is told with its reason) and the form shows what it kept.

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../shared/dom";
import { t } from "../shared/i18n";
import { showError } from "../shared/toast";
import type { AddonSettingsView, SettingSpec, SettingValue } from "../shared/types";

let addonId = "";
let savedTimer: number | undefined;

function control(spec: SettingSpec, value: SettingValue | undefined): string {
  const id = `as-${esc(spec.id)}`;
  const data = `id="${id}" data-setting="${esc(spec.id)}"`;
  switch (spec.type) {
    case "boolean":
      return `<input type="checkbox" role="switch" ${data}${value === true ? " checked" : ""} />`;
    case "number":
      return `<input type="number" step="1" min="${spec.min}" max="${spec.max}" inputmode="numeric" ${data} value="${esc(String(value ?? spec.default))}" />`;
    case "text":
      return `<input type="text" maxlength="${spec.max_length}" ${data} value="${esc(String(value ?? spec.default))}" />`;
    case "choice":
      return (
        `<select ${data}>` +
        spec.options
          .map((o) => `<option value="${esc(o.value)}"${o.value === value ? " selected" : ""}>${esc(o.label)}</option>`)
          .join("") +
        `</select>`
      );
  }
}

function draw(view: AddonSettingsView): void {
  $("addon-settings-form").innerHTML = view.specs
    .map(
      (spec) =>
        `<div class="setting"><div class="setting-text">` +
        `<label class="setting-name" for="as-${esc(spec.id)}">${esc(spec.label)}</label>` +
        (spec.hint ? `<p class="setting-hint">${esc(spec.hint)}</p>` : "") +
        `</div><div class="setting-control">${control(spec, view.values[spec.id])}</div></div>`,
    )
    .join("");
}

async function load(): Promise<void> {
  try {
    draw(await invoke<AddonSettingsView>("addon_settings", { id: addonId }));
  } catch (e) {
    showError(e);
  }
}

/** What a control says, as the value to keep: a switch is its tick, a number is a number (text that is not one is sent as it is, and refused). */
function valueOf(element: HTMLInputElement | HTMLSelectElement): SettingValue {
  if (element instanceof HTMLInputElement && element.type === "checkbox") return element.checked;
  if (element instanceof HTMLInputElement && element.type === "number") {
    const text = element.value.trim();
    return text !== "" && Number.isFinite(Number(text)) ? Number(text) : text;
  }
  return element.value;
}

async function keep(element: HTMLInputElement | HTMLSelectElement): Promise<void> {
  const key = element.dataset.setting;
  if (!key) return;
  try {
    await invoke("set_addon_setting", { id: addonId, key, value: valueOf(element) });
    const state = $("addon-settings-state");
    state.textContent = t("settings.saved");
    state.classList.add("shown");
    window.clearTimeout(savedTimer);
    savedTimer = window.setTimeout(() => state.classList.remove("shown"), 2000);
  } catch (e) {
    showError(e);
  }
  // What the core kept is what shows (a value that was refused puts the old one back).
  await load();
}

/** Opens the settings of an add-on. */
export async function openAddonSettings(id: string, name: string): Promise<void> {
  addonId = id;
  $("addon-settings-title").textContent = t("addon.settings_title", { name });
  $("addon-settings-state").classList.remove("shown");
  await load();
  const dialog = $<HTMLDialogElement>("dlg-addon-settings");
  if (!dialog.open) dialog.showModal();
}

export function setupAddonSettings(): void {
  $("addon-settings-close").addEventListener("click", () => $<HTMLDialogElement>("dlg-addon-settings").close());
  $("addon-settings-form").addEventListener("change", (ev) => {
    const target = ev.target as HTMLElement;
    if (target.matches("input[data-setting], select[data-setting]")) void keep(target as HTMLInputElement | HTMLSelectElement);
  });
}
