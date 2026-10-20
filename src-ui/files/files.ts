// Sending files to a device and opening them there: the messages about the
// sendings, dropping files on the window, and the question "which device?" for
// files dropped on it (a file manager's request has a small window of its own, see
// `request.ts`). The core does the work and announces it (`transfer` and `opened`
// events); this is only what the user sees of it. The device list itself stays the
// list of devices.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { describeError } from "../shared/errors";
import { t } from "../shared/i18n";
import { $, esc } from "../shared/dom";
import type { Device, OpenedUpdate, Snapshot, TransferUpdate } from "../shared/types";

/** What this part needs of the page around it. */
export interface Host {
  snapshot(): Snapshot | null;
  toast(level: "error" | "info", text: string, detail?: string): void;
  showError(e: unknown): void;
}

let host: Host;

/** The message of each sending that is shown, by its id. */
const live = new Map<number, HTMLElement>();

/** How long the end of a sending stays: briefly if it went well, long enough to read if not. */
const DONE_MS = 4000;
const FAILED_MS = 12000;

function deviceName(id: string): string {
  const d = host.snapshot()?.devices.find((x) => x.id === id);
  return d ? (d.label ?? d.name) : id;
}

function percent(u: TransferUpdate): number {
  return u.total === 0 ? 100 : Math.min(100, Math.floor((u.sent * 100) / u.total));
}

/** The sendings since the program started, by id, in the order they began in; shown on the page of the files. */
const history = new Map<number, TransferUpdate>();
const MAX_HISTORY = 200;

function renderHistory(): void {
  $("files-empty").hidden = history.size > 0;
  $("transfers-list").innerHTML = [...history.values()]
    .reverse()
    .map((u) => {
      const state =
        u.phase === "failed"
          ? `<span class="transfer-state bad">${esc(describeError(u.error ?? { code: "other", detail: "" }).text)}</span>`
          : u.phase === "done"
            ? `<span class="transfer-state ok">${esc(t("transfers.done"))}</span>`
            : `<span class="transfer-state">${percent(u)}%</span>`;
      const bar =
        u.phase === "sending" ? `<progress max="100" value="${percent(u)}" aria-label="${esc(u.name)}"></progress>` : "";
      return (
        `<li class="transfer ${u.phase}"><div class="transfer-line"><span class="transfer-name" title="${esc(u.name)}">${esc(u.name)}</span>` +
        `<span class="transfer-device">${esc(t("transfers.to", { device: deviceName(u.device) }))}</span>${state}</div>${bar}</li>`
      );
    })
    .join("");
}

/** A floating message for each file that is being sent: it follows the progress and then says how it ended. */
export function onTransfer(u: TransferUpdate): void {
  history.set(u.id, u);
  while (history.size > MAX_HISTORY) {
    const finished = [...history.values()].find((x) => x.phase !== "sending");
    if (!finished) break;
    history.delete(finished.id);
  }
  renderHistory();
  let el = live.get(u.id);
  if (!el) {
    el = document.createElement("div");
    const mine = el;
    mine.addEventListener("click", () => {
      mine.remove();
      live.delete(u.id);
    });
    $("toasts").appendChild(mine);
    live.set(u.id, mine);
  }
  const where = esc(t("transfers.to", { device: deviceName(u.device) }));
  if (u.phase === "failed") {
    const { text, detail } = describeError(u.error ?? { code: "other", detail: "" });
    el.className = "toast error transfer";
    el.innerHTML = `<strong>${esc(u.name)}</strong> <span>${where}</span><div>${esc(text)}</div>${detail ? `<pre>${esc(detail)}</pre>` : ""}`;
  } else {
    el.className = "toast transfer";
    const state = u.phase === "done" ? esc(t("transfers.done")) : `${percent(u)}%`;
    const bar = u.phase === "done" ? "" : `<progress max="100" value="${percent(u)}" aria-label="${esc(u.name)}"></progress>`;
    el.innerHTML = `<strong>${esc(u.name)}</strong> <span>${where} · ${state}</span>${bar}`;
  }
  if (u.phase !== "sending") {
    const ended = el;
    setTimeout(() => {
      ended.remove();
      live.delete(u.id);
    }, u.phase === "done" ? DONE_MS : FAILED_MS);
  }
}

export function onOpened(update: OpenedUpdate): void {
  const name = update.file.split(/[\\/]/).pop() ?? update.file;
  if (update.error) {
    const { text, detail } = describeError(update.error);
    host.toast("error", t("opened.failed", { name, reason: text }), detail);
  } else {
    host.toast("info", t("opened.done", { name, device: deviceName(update.device) }));
  }
}

// ---------------------------------------------------------------- the question

/** Asks which of the devices that are ready the files are sent to. */
function ask(title: string, files: string[], onPick: (device: string) => void): void {
  const dlg = $<HTMLDialogElement>("dlg-choose");
  const online = (host.snapshot()?.devices ?? []).filter((d: Device) => d.state === "online");
  $("choose-title").textContent = title;
  const names = files.map((f) => f.split(/[\\/]/).pop() ?? f);
  $("choose-files").textContent =
    names.length <= 3 ? names.join(", ") : `${names.slice(0, 3).join(", ")} ${t("chooser.and_more", { count: names.length - 3 })}`;
  const list = $("choose-list");
  list.innerHTML = online
    .map(
      (d) =>
        `<li><button type="button" data-device="${esc(d.id)}"><span class="choice-name">${esc(d.label ?? d.name)}</span></button></li>`,
    )
    .join("");
  if (online.length === 0) list.innerHTML = `<li class="hint">${esc(t("chooser.no_devices"))}</li>`;
  list.querySelectorAll<HTMLButtonElement>("button[data-device]").forEach((b) => {
    b.addEventListener("click", () => {
      dlg.close();
      onPick(b.dataset.device!);
    });
  });
  if (!dlg.open) dlg.showModal();
}

// ---------------------------------------------------------------- dropping

async function send(device: string, files: string[]): Promise<void> {
  try {
    await invoke("send_files", { device, files });
  } catch (e) {
    host.showError(e);
  }
}

/** The online card under a point of the window, if any. */
function cardAt(position: { x: number; y: number }): string | null {
  const ratio = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(position.x / ratio, position.y / ratio);
  const card = el?.closest<HTMLElement>(".card[data-device]");
  return card && card.dataset.state === "online" ? (card.dataset.device ?? null) : null;
}

function markTarget(device: string | null): void {
  document.querySelectorAll<HTMLElement>(".card.drop-target").forEach((c) => c.classList.remove("drop-target"));
  if (device === null) return;
  document.querySelector<HTMLElement>(`.card[data-device="${CSS.escape(device)}"]`)?.classList.add("drop-target");
}

function dropped(paths: string[], position: { x: number; y: number }): void {
  if (paths.length === 0) return;
  const target = cardAt(position);
  if (target) {
    void send(target, paths);
    return;
  }
  const online = (host.snapshot()?.devices ?? []).filter((d) => d.state === "online");
  if (online.length === 0) {
    host.toast("error", t("drop.no_device"));
  } else if (online.length === 1) {
    void send(online[0].id, paths);
  } else {
    ask(t("chooser.title_send"), paths, (device) => void send(device, paths));
  }
}

export async function initFiles(h: Host): Promise<void> {
  host = h;
  $("transfers-clear").addEventListener("click", () => {
    for (const [id, u] of history) if (u.phase !== "sending") history.delete(id);
    renderHistory();
  });
  $("choose-cancel").addEventListener("click", () => $<HTMLDialogElement>("dlg-choose").close());
  try {
    await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      switch (p.type) {
        case "enter":
        case "over":
          document.body.classList.add("dragging");
          markTarget(cardAt(p.position));
          break;
        case "leave":
          document.body.classList.remove("dragging");
          markTarget(null);
          break;
        case "drop":
          document.body.classList.remove("dragging");
          markTarget(null);
          dropped(p.paths, p.position);
          break;
      }
    });
  } catch {
    // Not running inside Tauri (plain browser preview).
  }
}
