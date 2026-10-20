// What is set for this device alone (the "device settings" tab): a change is kept as soon as it is made.

import { invoke } from "@tauri-apps/api/core";
import { $ } from "../../shared/dom";
import { t } from "../../shared/i18n";
import { wholeNumberOrNull } from "../../shared/numbers";
import { showError } from "../../shared/toast";
import { state } from "./state";

/** What is set for this device alone (the core keeps it; `null` follows the general setting). */
interface DeviceSettings {
  keyboard_mode: "uhid" | "sdk" | null;
  video_bit_rate_mbps: number | null;
  max_size: number;
  apps_in_virtual_display: boolean;
  keep_awake: boolean;
}

let savedStateTimer: number | undefined;

/** Puts what the core keeps into the controls. */
export async function loadDeviceSettings(): Promise<void> {
  try {
    const s = await invoke<DeviceSettings>("device_settings", { device: state.deviceId });
    $<HTMLSelectElement>("ds-keyboard").value = s.keyboard_mode ?? "";
    $<HTMLInputElement>("ds-bit-rate").value = s.video_bit_rate_mbps === null ? "" : String(s.video_bit_rate_mbps);
    $<HTMLInputElement>("ds-max-size").value = String(s.max_size);
    $<HTMLInputElement>("ds-virtual").checked = s.apps_in_virtual_display;
    $<HTMLInputElement>("ds-awake").checked = s.keep_awake;
  } catch (e) {
    showError(e);
  }
}

/** What is set for the device is kept when it is changed; a number when it is left. */
export function wireDeviceSettings(): void {
  $("tab-settings").addEventListener("change", async (ev) => {
    if (!(ev.target as HTMLElement).matches("select, input")) return;
    const keyboard = $<HTMLSelectElement>("ds-keyboard").value;
    const settings: DeviceSettings = {
      keyboard_mode: keyboard === "uhid" || keyboard === "sdk" ? keyboard : null,
      video_bit_rate_mbps: wholeNumberOrNull($<HTMLInputElement>("ds-bit-rate").value, 1, 200),
      max_size: wholeNumberOrNull($<HTMLInputElement>("ds-max-size").value, 0, 16384) ?? 0,
      apps_in_virtual_display: $<HTMLInputElement>("ds-virtual").checked,
      keep_awake: $<HTMLInputElement>("ds-awake").checked,
    };
    const saved = $("ds-state");
    try {
      await invoke("set_device_settings", { device: state.deviceId, settings });
      // What the core made of it (a number that is not allowed is the general setting) is what shows.
      await loadDeviceSettings();
      saved.textContent = t("settings.saved");
      saved.classList.add("shown");
      window.clearTimeout(savedStateTimer);
      savedStateTimer = window.setTimeout(() => saved.classList.remove("shown"), 2000);
    } catch (e) {
      showError(e);
      await loadDeviceSettings();
    }
  });
}
