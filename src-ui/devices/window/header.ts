// The top of the window of a device: its name (which can be renamed in place), whether it is usable, the buttons
// (open the screen, read it again) and the buttons the add-ons put there.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { $ } from "../../shared/dom";
import { t } from "../../shared/i18n";
import { SCREEN_ICON } from "../../shared/icons";
import { showError } from "../../shared/toast";
import type { MenuItem } from "../../shared/types";
import { renderStatusStrip } from "./info";
import { current, state } from "./state";

const PENCIL_ICON =
  `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4Z"/><path d="M13.5 6.5l4 4"/></svg>`;

export function renderHeader(): void {
  const d = current();
  const name = d ? (d.label ?? d.name) : state.deviceId;
  $("dev-name").textContent = name;
  document.title = name;
  getCurrentWindow().setTitle(name).catch(() => {});
  const light = $("dev-state");
  const words = t(`state.${d?.state ?? "disconnected"}`);
  light.className = "light";
  light.dataset.state = d?.state ?? "disconnected";
  light.title = words;
  light.setAttribute("aria-label", words);
  // The reported name is shown next to a name of the user's own.
  const reported = $("dev-reported");
  reported.textContent = d?.label ? t("device_window.reported_as", { name: d.name }) : "";
  reported.hidden = !d?.label;

  const rename = $("dev-rename-start");
  rename.title = t("device_window.rename");
  rename.setAttribute("aria-label", t("device_window.rename"));

  const online = d?.state === "online";
  const running = state.snap?.sessions.includes(state.deviceId) ?? false;
  const open = $<HTMLButtonElement>("dev-open");
  open.disabled = !online || running;
  const label = t(running ? "device.running" : "device.open");
  open.title = label;
  open.setAttribute("aria-label", label);
  $<HTMLButtonElement>("dev-refresh").disabled = !online;

  const note = $("dev-note");
  note.textContent = !d ? t("device_window.gone") : t("device_window.not_online");
  note.hidden = online;
  renderStatusStrip();
}

/** The buttons the add-ons put in the menu of the device. */
export async function renderAddons(): Promise<void> {
  const box = $("dev-addons");
  let items: MenuItem[] = [];
  try {
    items = await invoke<MenuItem[]>("device_menu", { device: state.deviceId });
  } catch {
    // The device is not in the list any more.
  }
  box.innerHTML = "";
  for (const item of items) {
    // The built-in entry is the app list below.
    if (item.id === "open_app") continue;
    const b = document.createElement("button");
    b.type = "button";
    b.disabled = !item.enabled;
    b.textContent = item.label ?? "";
    b.addEventListener("click", () => {
      invoke("run_menu_item", { device: state.deviceId, item: item.id }).catch(showError);
    });
    box.appendChild(b);
  }
}

/**
 * Renaming in place: the pencil turns the name into a box that has the name the
 * device reports as its placeholder. Enter or leaving the box keeps what was typed,
 * Escape drops it, and an empty box takes the name of the user away (the device
 * is shown by the name it reports again).
 */
function wireRename(): void {
  const title = $("dev-name");
  const pencil = $("dev-rename-start");
  const input = $<HTMLInputElement>("dev-name-input");
  let editing = false;

  const stop = () => {
    editing = false;
    input.hidden = true;
    title.hidden = false;
    pencil.hidden = false;
  };
  const start = () => {
    const d = current();
    editing = true;
    input.value = d?.label ?? "";
    input.placeholder = d?.name ?? state.deviceId;
    title.hidden = true;
    pencil.hidden = true;
    input.hidden = false;
    input.focus();
    input.select();
  };
  const keep = async () => {
    if (!editing) return;
    const text = input.value.trim();
    const label = text === "" ? null : text;
    stop();
    if (label === (current()?.label ?? null)) return;
    try {
      await invoke("set_device_label", { device: state.deviceId, label });
    } catch (e) {
      showError(e);
    }
  };

  pencil.addEventListener("click", start);
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      void keep();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      stop();
    }
  });
  input.addEventListener("blur", () => void keep());
}

/** Wires the buttons of the header; `onRefresh` is what is read again besides the device itself. */
export function setupHeader(onRefresh: () => void): void {
  $("dev-open").innerHTML = SCREEN_ICON;
  $("dev-rename-start").innerHTML = PENCIL_ICON;
  $("dev-open").addEventListener("click", () => {
    invoke("open_device", { id: state.deviceId }).catch(showError);
  });
  $("dev-refresh").addEventListener("click", () => {
    // The core reads the device now; the reading comes back as an event.
    invoke("refresh_device_info", { device: state.deviceId }).catch(showError);
    onRefresh();
  });
  wireRename();
}
