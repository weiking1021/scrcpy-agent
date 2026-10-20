// The list of the devices on the main page: the cards in two groups (the devices this computer knows, and
// the ones that are only seen around), what each card offers to do, and the latest reading of each usable device.

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../shared/dom";
import { t } from "../shared/i18n";
import { SCREEN_ICON } from "../shared/icons";
import { statusStrip } from "../shared/status";
import { showError } from "../shared/toast";
import { isCoreError } from "../shared/types";
import type { Device, DeviceInfo, Snapshot } from "../shared/types";
import { openPairDialog } from "./pairing";

/** What this part needs of the page around it. */
export interface ListHost {
  snapshot(): Snapshot | null;
  /** Draws the whole page again (the list, and what depends on it). */
  render(): void;
}

let host: ListHost;

/** Devices with a connect request in flight, to avoid double clicks. */
const busy = new Set<string>();

/** The latest reading of each usable device. The core makes the readings and announces them. */
let infos: Record<string, DeviceInfo> = {};

const INFO_ICON =
  `<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.8h.01"/></svg>`;

/** Shows a new reading on the card of its device without rebuilding the list. */
export function showInfo(id: string, info: DeviceInfo): void {
  infos[id] = info;
  const strip = document.querySelector<HTMLElement>(`[data-status="${CSS.escape(id)}"]`);
  const device = host.snapshot()?.devices.find((d) => d.id === id) ?? null;
  if (strip) strip.innerHTML = statusStrip(device, info, true);
}

/** Takes in the readings made before this window was listening (the ones heard since are newer and stay). */
export async function loadInfos(): Promise<void> {
  infos = { ...(await invoke<Record<string, DeviceInfo>>("device_infos")), ...infos };
}

function deviceCard(d: Device, running: boolean): string {
  const title = d.label ?? d.name;
  // The serial and the Android version are what tells devices apart; how it is
  // connected is in the device's window. A device that is not connected yet shows
  // the address it is to be connected at.
  const sub: string[] = [d.id];
  if (d.android) sub.push(t("device.android", { version: d.android }));
  const address = d.state !== "online" && d.address ? d.address : null;

  const notes: string[] = [];
  if (d.state === "unauthorized") notes.push(t("device.unauthorized_hint"));
  if (d.state === "online" && d.sdk !== null && d.sdk < 30) notes.push(t("device.no_audio"));

  let actions = "";
  if (d.state === "online") {
    // The green button opens the screen; it only shows a screen, its text is the tooltip.
    const label = esc(running ? t("device.running") : t("device.open"));
    actions +=
      `<button class="icon open-screen" data-action="open" data-id="${esc(d.id)}"` +
      ` title="${label}" aria-label="${label}"${running ? " disabled" : ""}>${SCREEN_ICON}</button>` +
      `<button class="with-icon" data-action="details" data-id="${esc(d.id)}">${INFO_ICON}<span>${esc(t("card.details"))}</span></button>`;
  } else if ((d.state === "discovered" || d.state === "unauthorized") && d.address) {
    const pending = busy.has(d.address);
    const label = d.state === "unauthorized" ? t("device.request_again") : t("device.connect");
    actions += `<button class="primary" data-action="connect" data-address="${esc(d.address)}"${pending ? " disabled" : ""}>${esc(label)}</button>`;
  }
  if (d.known && d.state !== "online") {
    actions += `<button data-action="forget" data-id="${esc(d.id)}">${esc(t("device.forget"))}</button>`;
  }

  return `
    <article class="card" data-state="${d.state}" data-device="${esc(d.id)}">
      <div class="card-head">
        <h3>${esc(title)}</h3>
        <span class="light" data-state="${d.state}" role="img" title="${esc(t(`state.${d.state}`))}" aria-label="${esc(t(`state.${d.state}`))}"></span>
      </div>
      <p class="serial">${esc(sub.join(" · "))}</p>
      ${address ? `<p class="address">${esc(address)}</p>` : ""}
      ${d.state === "online" ? `<div class="status-strip compact" role="list" data-status="${esc(d.id)}">${statusStrip(d, infos[d.id], true)}</div>` : ""}
      ${notes.map((n) => `<p class="note">${esc(n)}</p>`).join("")}
      <div class="card-actions">${actions}</div>
    </article>`;
}

// ---------------------------------------------------------------- the groups of the list

/** The list is in two groups: the devices this computer knows (paired), and the others, which are only seen around. */
const DEVICE_GROUPS = ["paired", "strangers"] as const;
type DeviceGroup = (typeof DEVICE_GROUPS)[number];

/** The groups the user folded; kept for the next time (it is only a convenience). */
const FOLDED_KEY = "folded-device-groups";

function foldedGroups(): Set<DeviceGroup> {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(FOLDED_KEY) ?? "[]");
    return new Set(DEVICE_GROUPS.filter((g) => Array.isArray(saved) && saved.includes(g)));
  } catch {
    return new Set();
  }
}

function saveFoldedGroups(folded: Set<DeviceGroup>): void {
  try {
    localStorage.setItem(FOLDED_KEY, JSON.stringify([...folded]));
  } catch {
    // Without storage the groups are open at the next start.
  }
}

/** Which group a device is in: the known ones are paired. */
function groupOf(d: Device): DeviceGroup {
  return d.known ? "paired" : "strangers";
}

/** What is usable comes first, then what asks for a decision, then the rest. */
const STATE_ORDER: Record<Device["state"], number> = { online: 0, unauthorized: 1, discovered: 2, offline: 3, disconnected: 4 };

function deviceGroup(snap: Snapshot, group: DeviceGroup, running: Set<string>): string {
  const members = snap.devices
    .filter((d) => groupOf(d) === group)
    .sort((a, b) => STATE_ORDER[a.state] - STATE_ORDER[b.state]);
  const folded = foldedGroups().has(group);
  const title = t(`devices.group_${group}`, { count: members.length });
  return (
    `<section class="device-group" data-group="${group}">` +
    `<button type="button" class="group-toggle" data-group-toggle="${group}" aria-expanded="${!folded}">` +
    `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 10l5 5 5-5"/></svg><span>${esc(title)}</span></button>` +
    (folded
      ? ""
      : members.length === 0
        ? `<p class="group-empty">${esc(t("devices.group_empty"))}</p>`
        : `<div class="grid">${members.map((d) => deviceCard(d, running.has(d.id))).join("")}</div>`) +
    `</section>`
  );
}

function toggleGroup(group: DeviceGroup): void {
  const folded = foldedGroups();
  if (!folded.delete(group)) folded.add(group);
  saveFoldedGroups(folded);
  host.render();
}

/** Draws the list for this state of the program. */
export function renderDeviceList(snap: Snapshot): void {
  const running = new Set(snap.sessions);
  // A device that is no longer usable has no reading (the core drops it too).
  for (const id of Object.keys(infos)) {
    if (!snap.devices.some((d) => d.id === id && d.state === "online")) delete infos[id];
  }
  $("devices").innerHTML = DEVICE_GROUPS.map((group) => deviceGroup(snap, group, running)).join("");
  $("empty").hidden = snap.devices.length > 0;
}

// ---------------------------------------------------------------- actions

async function onListClick(ev: MouseEvent): Promise<void> {
  const toggle = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-group-toggle]");
  const group = DEVICE_GROUPS.find((g) => g === toggle?.dataset.groupToggle);
  if (group) {
    toggleGroup(group);
    return;
  }
  const btn = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-action]");
  if (!btn) return;
  const { action, id, address } = btn.dataset;
  try {
    if (action === "open" && id) {
      await invoke("open_device", { id });
    } else if (action === "details" && id) {
      await invoke("open_device_window", { device: id });
    } else if (action === "forget" && id) {
      await invoke("forget_device", { id });
    } else if (action === "connect" && address) {
      busy.add(address);
      host.render();
      try {
        await invoke("connect_device", { address });
      } catch (e) {
        // The device answered but turned adb away: it is not paired yet.
        if (!isCoreError(e) || e.code !== "not_paired") throw e;
        openPairDialog(t("pair.not_paired"));
      } finally {
        busy.delete(address);
        host.render();
      }
    }
  } catch (e) {
    showError(e);
  }
}

export function setupDeviceList(next: ListHost): void {
  host = next;
  $("devices").addEventListener("click", onListClick);
}
