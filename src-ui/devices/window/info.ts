// What is known about the device, in the "info" tab, and the strip of small icons in the header. The core reads
// the devices again on its own (see the setting `device_refresh_seconds`); this window only shows what it announces
// (`device_info` events).

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../../shared/dom";
import { t } from "../../shared/i18n";
import { batteryText, bytes, statusStrip } from "../../shared/status";
import type { DeviceDetails } from "../../shared/types";
import { duration, moment } from "./format";
import { current, state } from "./state";

const dash = "—";

type Row = [label: string, value: string | null | undefined];

function section(title: string, rows: Row[]): string {
  const shown = rows.filter(([, v]) => v !== undefined);
  // A name on the left in a quiet colour, its value on the right, a line between rows.
  return (
    `<section class="kv-group"><h2 class="kv-title">${esc(title)}</h2><div class="kv">` +
    shown
      .map(
        ([k, v]) =>
          `<div class="kv-row"><span class="kv-key">${esc(k)}</span><span class="kv-value${v === null ? " none" : ""}">${esc(v ?? dash)}</span></div>`,
      )
      .join("") +
    `</div></section>`
  );
}

/** The strip of small icons under the name of the device. */
export function renderStatusStrip(): void {
  $("dev-status").innerHTML = statusStrip(current(), state.details?.info, false);
}

export function renderInfo(): void {
  renderStatusStrip();
  const el = $("dev-info");
  const details = state.details;
  if (!details) {
    el.innerHTML = "";
    return;
  }
  const d = details.device;
  const i = details.info;
  const sys = i?.system;
  const connection: Row[] = [
    [t("device_window.id"), d.id],
    [t("device_window.transport"), d.transport ? t(`transport.${d.transport}`) : null],
    [t("device_window.address"), d.address],
    [t("device_window.first_seen"), moment(details.first_seen)],
    [t("device_window.last_seen"), moment(details.last_seen)],
  ];
  const system: Row[] = [
    [t("device_window.reported_name"), details.reported_name],
    [t("device_window.manufacturer"), sys?.manufacturer ?? null],
    [t("device_window.brand"), sys?.brand ?? null],
    [t("device_window.model"), sys?.model ?? null],
    [t("device_window.codename"), sys?.device ?? null],
    [t("device_window.soc"), sys?.soc ?? null],
    [t("device_window.android"), sys?.android ? `${sys.android}${sys.sdk !== null ? ` (SDK ${sys.sdk})` : ""}` : (d.android ?? null)],
    [t("device_window.security_patch"), sys?.security_patch ?? null],
    [t("device_window.build"), sys?.build ?? null],
    [t("device_window.abi"), sys?.abi ?? null],
    [t("device_window.locale"), sys?.locale ?? null],
  ];
  let html = section(t("device_window.section_connection"), connection);
  html += section(t("device_window.section_system"), system);
  if (i) {
    const s = i.screen;
    html += section(t("device_window.section_screen"), [
      [
        t("device_window.screen_now"),
        s ? `${s.width} × ${s.height} (${t(s.width > s.height ? "device_window.landscape" : "device_window.portrait")})` : null,
      ],
      [t("device_window.screen_panel"), s ? `${s.physical_width} × ${s.physical_height}` : null],
      [t("device_window.density"), s?.density != null ? `${s.density} dpi` : null],
    ]);
    html += section(t("device_window.section_state"), [
      [t("device_window.battery"), i.battery ? batteryText(i.battery) : null],
      [t("device_window.battery_temperature"), i.battery?.temperature != null ? `${(i.battery.temperature / 10).toFixed(1)} °C` : null],
      [t("device_window.battery_voltage"), i.battery?.voltage != null ? `${(i.battery.voltage / 1000).toFixed(2)} V` : null],
      [
        t("device_window.storage"),
        i.storage ? t("device_window.of", { used: bytes(i.storage.used_kb), total: bytes(i.storage.total_kb) }) : null,
      ],
      [t("device_window.storage_free"), i.storage ? bytes(i.storage.available_kb) : null],
      [
        t("device_window.memory"),
        i.memory ? t("device_window.memory_value", { free: bytes(i.memory.available_kb), total: bytes(i.memory.total_kb) }) : null,
      ],
      [t("device_window.uptime"), i.uptime_seconds !== null ? duration(i.uptime_seconds) : null],
    ]);
  }
  el.innerHTML = html;
}

/** Increases with every request, so a slow answer of an earlier one is dropped. */
let detailsRequest = 0;

/** Asks the core for what it knows about the device and shows it. */
export async function loadDetails(): Promise<void> {
  const request = ++detailsRequest;
  try {
    const result = await invoke<DeviceDetails>("device_details", { device: state.deviceId });
    if (request !== detailsRequest) return;
    state.details = result;
  } catch {
    if (request !== detailsRequest) return;
    state.details = null;
  }
  renderInfo();
}
