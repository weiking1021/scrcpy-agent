// How a device's state is shown: small icons with one short value each. The
// device list and the window of a device show it the same way, from the latest
// reading the core made (the core reads the devices, not the windows).

import { esc } from "./dom";
import { t } from "./i18n";
import type { Battery, Device, DeviceInfo } from "./types";

/** Kilobytes as KB, MB, GB or TB. */
export function bytes(kb: number): string {
  const units = ["KB", "MB", "GB", "TB"];
  let value = kb;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

/** "49% · discharging · USB". */
export function batteryText(b: Battery): string {
  const percent = Math.round((b.level * 100) / b.scale);
  const parts = [`${percent}%`];
  if (b.status) parts.push(t(`battery.${b.status}`));
  const sources: string[] = [];
  if (b.ac) sources.push(t("battery.ac"));
  if (b.usb) sources.push(t("battery.usb"));
  if (b.wireless) sources.push(t("battery.wireless"));
  if (sources.length > 0) parts.push(sources.join(" + "));
  return parts.join(" · ");
}

const icon = (body: string): string =>
  `<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const ICON_WIFI = icon(`<path d="M5 12.5a10 10 0 0 1 14 0M8.5 16a5 5 0 0 1 7 0"/><path d="M12 19.5h.01"/>`);
const ICON_USB = icon(`<path d="M12 3v13M9 7l3-4 3 4"/><circle cx="12" cy="19" r="2"/><path d="M12 12l-4-2v-2M12 14l4-2v-2"/>`);
const ICON_STORAGE = icon(`<rect x="3" y="6.5" width="18" height="11" rx="2"/><path d="M3 13.5h18M7 16h.01"/>`);
const ICON_MEMORY = icon(`<rect x="6" y="6" width="12" height="12" rx="1.5"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>`);

/** What is shown for a value that is not known. */
const UNKNOWN = "--";

/** A battery with nothing in it: for a level that is not known. */
const ICON_BATTERY = icon(`<rect x="2" y="7" width="18" height="10" rx="2"/><path d="M22 11v2"/>`);

function batteryIcon(b: Battery): string {
  const fill = Math.max(0, Math.min(14, Math.round((14 * b.level) / b.scale)));
  const bolt = b.status === "charging" ? `<path d="M12.5 8.5l-3 4h4l-3 4" stroke-width="1.5"/>` : "";
  return icon(
    `<rect x="2" y="7" width="18" height="10" rx="2"/><path d="M22 11v2"/><rect x="4" y="9" width="${fill}" height="6" rx="0.5" fill="currentColor" stroke="none" opacity="0.55"/>${bolt}`,
  );
}

function screenIcon(landscape: boolean): string {
  return landscape
    ? icon(`<rect x="3" y="6" width="18" height="12" rx="2"/>`)
    : icon(`<rect x="6" y="3" width="12" height="18" rx="2"/>`);
}

interface Chip {
  icon: string;
  text: string;
  tip: string;
  /** The colour says how it is: good, to watch, or bad. No colour is neutral. */
  level?: "ok" | "warn" | "bad";
  /** The one that matters most (the battery): it stands out, the others are plain. */
  primary?: boolean;
}

/**
 * The chips of a device. `compact` is for the device list: only the battery,
 * the storage and which way the screen is held, each with its icon and value
 * (the screen with the icon alone).
 */
export function statusStrip(device: Device | null, info: DeviceInfo | null | undefined, compact: boolean): string {
  const chips: Chip[] = [];
  if (!compact && device?.transport) {
    const name = t(`transport.${device.transport}`);
    chips.push({
      icon: device.transport === "usb" ? ICON_USB : ICON_WIFI,
      text: name,
      tip: device.address ? `${name} · ${device.address}` : name,
    });
  }
  // What cannot be told (nothing was read, or the device does not say) is shown as `--` where it would be, so the place
  // is the same either way (an empty place would move what is below it when the readings come).
  const online = device?.state === "online";
  const unknown = (icon: string, what: string, primary = false): Chip => ({
    icon,
    text: UNKNOWN,
    tip: `${t(what)}: ${UNKNOWN}`,
    primary,
  });
  if (!info?.battery && online) chips.push(unknown(ICON_BATTERY, "device_window.battery", true));
  if (info?.battery) {
    const b = info.battery;
    const percent = Math.round((b.level * 100) / b.scale);
    chips.push({
      icon: batteryIcon(b),
      text: `${percent}%`,
      tip: `${t("device_window.battery")}: ${batteryText(b)}`,
      // The battery is what matters most, so it is the one that is always coloured.
      level: b.status === "charging" || percent > 50 ? "ok" : percent > 20 ? "warn" : "bad",
      primary: true,
    });
  }
  if (!info?.storage && online) chips.push(unknown(ICON_STORAGE, "device_window.storage"));
  if (info?.storage) {
    const percent = Math.round((info.storage.used_kb * 100) / info.storage.total_kb);
    chips.push({
      icon: ICON_STORAGE,
      text: `${percent}%`,
      tip: `${t("device_window.storage")}: ${t("device_window.of", { used: bytes(info.storage.used_kb), total: bytes(info.storage.total_kb) })}`,
      // Only when it is nearly full: a colour here must not pull the eye from the battery.
      level: percent >= 95 ? "warn" : undefined,
    });
  }
  if (!compact && !info?.memory && online) chips.push(unknown(ICON_MEMORY, "device_window.memory"));
  if (!compact && info?.memory) {
    const free = Math.round((info.memory.available_kb * 100) / info.memory.total_kb);
    chips.push({
      icon: ICON_MEMORY,
      text: `${free}%`,
      tip: `${t("device_window.memory")}: ${t("device_window.memory_value", { free: bytes(info.memory.available_kb), total: bytes(info.memory.total_kb) })}`,
      level: free < 10 ? "warn" : undefined,
    });
  }
  if (info?.screen) {
    const s = info.screen;
    const landscape = s.width > s.height;
    chips.push({
      icon: screenIcon(landscape),
      text: compact ? "" : `${s.width}×${s.height}`,
      tip: `${t("device_window.screen_now")}: ${s.width} × ${s.height} (${t(landscape ? "device_window.landscape" : "device_window.portrait")})`,
    });
  }
  return chips
    .map(
      (c) =>
        `<span class="chip${c.primary ? " primary" : ""}${c.level ? ` ${c.level}` : ""}" role="listitem" title="${esc(c.tip)}" aria-label="${esc(c.tip)}">${c.icon}${c.text ? `<span class="value">${esc(c.text)}</span>` : ""}</span>`,
    )
    .join("");
}
