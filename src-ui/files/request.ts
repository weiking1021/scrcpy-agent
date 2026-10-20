// The small window of a request from a file manager ("send to / open on a device"):
// it asks which device when there is none, then shows how each file is doing, and
// closes itself a moment after everything went well. The core keeps the state and
// decides what follows from it (`core::request`); this only draws it and sends the
// user's choices back. The main window is not involved.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { describeError } from "../shared/errors";
import { loadUserPacks, setAppName, setLanguage, t, translateDom } from "../shared/i18n";
import { suppressNativeContextMenu } from "../shared/contextmenu";
import { $, esc } from "../shared/dom";
import { initRipple } from "../shared/ripple";
import { initTheme } from "../shared/theme";
import { supportText } from "./support";
import type { CoreEvent, Device, OpenSupport, RequestItem, RequestView, Snapshot } from "../shared/types";

let devices: Device[] = [];
let view: RequestView | null = null;
/** Closes the window after the end, unless the view changes first. */
let closeTimer: number | undefined;
/** The device list is drawn again only for another request, or when the devices change. */
let drawnChoiceOf: number | null = null;

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

const deviceName = (id: string | null) => {
  const d = devices.find((x) => x.id === id);
  return d ? (d.label ?? d.name) : (id ?? "");
};

const opens = (v: RequestView) => v.action === "open_default" || v.action === "open_choose";

function showError(e: unknown): void {
  const { text, detail } = describeError(e);
  $("req-status").textContent = detail ? `${text} ${detail}` : text;
}

function stateText(item: RequestItem): string {
  switch (item.state) {
    case "waiting":
      return t("request.waiting");
    case "working":
      return t("request.working");
    case "done":
      return t("request.done");
    case "failed": {
      const { text, detail } = describeError(item.error);
      return detail ? `${text} (${detail})` : text;
    }
  }
}

/** The list of the devices that are ready, each with what it says about opening the (first) file. */
function drawChoice(v: RequestView): void {
  drawnChoiceOf = v.id;
  const online = devices.filter((d) => d.state === "online");
  const list = $("req-list");
  const check = opens(v);
  list.innerHTML =
    online.length === 0
      ? `<li class="hint">${esc(t("chooser.no_devices"))}</li>`
      : online
          .map(
            (d) =>
              `<li><button type="button" data-device="${esc(d.id)}"${check ? " disabled" : ""}>` +
              `<span class="choice-name">${esc(d.label ?? d.name)}</span>` +
              `<span class="choice-state" data-state-of="${esc(d.id)}">${check ? esc(t("chooser.checking")) : ""}</span></button></li>`,
          )
          .join("");
  list.querySelectorAll<HTMLButtonElement>("button[data-device]").forEach((b) => {
    b.addEventListener("click", () => {
      invoke("resolve_choice", { device: b.dataset.device }).catch(showError);
    });
  });
  if (!check) return;
  // Each device is asked on its own; the answers fill in as they come.
  for (const b of list.querySelectorAll<HTMLButtonElement>("button[data-device]")) {
    const id = b.dataset.device!;
    void (async () => {
      let support: OpenSupport = { kind: "unreachable" };
      try {
        support = await invoke<OpenSupport>("open_support", { device: id, file: v.items[0]?.file ?? "" });
      } catch {
        // The device went away while asking.
      }
      if (view?.id !== v.id) return;
      const { text, usable } = supportText(support);
      const label = list.querySelector<HTMLElement>(`[data-state-of="${CSS.escape(id)}"]`);
      if (label) label.textContent = text;
      b.disabled = !usable;
    })();
  }
}

function drawProgress(v: RequestView): void {
  drawnChoiceOf = null;
  $("req-list").innerHTML = v.items
    .map(
      (item) =>
        `<li class="req-item ${item.state}"><span class="choice-name">${esc(fileName(item.file))}</span>` +
        `<span class="choice-state">${esc(stateText(item))}</span></li>`,
    )
    .join("");
}

function render(): void {
  window.clearTimeout(closeTimer);
  const v = view;
  if (v === null) return;
  const open = opens(v);
  $("req-title").textContent = t(
    v.phase === "choosing" ? (open ? "chooser.title_open" : "chooser.title_send") : open ? "request.title_open" : "request.title_send",
  );
  const names = v.items.map((i) => fileName(i.file));
  $("req-files").textContent =
    names.length <= 3 ? names.join(", ") : `${names.slice(0, 3).join(", ")} ${t("chooser.and_more", { count: names.length - 3 })}`;
  const note = $("req-note");
  note.hidden = !(v.phase === "choosing" && v.no_default);
  note.textContent = t("chooser.no_default");

  if (v.phase === "choosing") {
    if (drawnChoiceOf !== v.id) drawChoice(v);
    $("req-status").textContent = "";
  } else {
    drawProgress(v);
    $("req-status").textContent =
      v.device === null ? "" : t(open ? "request.open_on" : "request.sent_to", { device: deviceName(v.device) });
  }

  // Cancelling a question drops the request; a finished one is only closed.
  const button = $<HTMLButtonElement>("req-button");
  button.textContent = t(v.phase === "choosing" ? "common.cancel" : "request.close");
  button.onclick = () => {
    invoke(v.phase === "choosing" ? "cancel_choice" : "dismiss_request", { id: v.id }).catch(showError);
  };

  // When everything went well the window goes by itself; the core says after how long.
  if (v.close_after_ms !== null) {
    $("req-status").textContent += ` ${t("request.closing")}`;
    closeTimer = window.setTimeout(() => {
      invoke("dismiss_request", { id: v.id }).catch(showError);
    }, v.close_after_ms);
  }
}

function setView(next: RequestView | null): void {
  view = next;
  render();
}

async function main(): Promise<void> {
  initRipple();
  void initTheme();
  suppressNativeContextMenu();
  await loadUserPacks();
  const snapshot = await invoke<Snapshot>("get_snapshot");
  devices = snapshot.devices;
  setLanguage(snapshot.settings.language);
  setAppName(snapshot.settings.app_name);
  translateDom();
  await getCurrentWindow().setTitle(t("request.window_title"));

  // Listen first, then ask what there is now: an event that came in between is newer.
  await listen<CoreEvent>("core-event", (e) => {
    const ev = e.payload;
    if (ev.kind === "request") setView(ev.data);
    else if (ev.kind === "state") {
      devices = ev.data.devices;
      // Devices that came or went change the question.
      if (view?.phase === "choosing") {
        drawnChoiceOf = null;
        render();
      }
    }
  });
  setView(await invoke<RequestView | null>("current_request"));
}

main().catch(showError);
