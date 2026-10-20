// Pairing a device: the dialog that asks for the address and the code shown on the phone, and the list of the
// pairing services found on the network (a button for each, to take its address).

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../shared/dom";
import { describeError } from "../shared/errors";
import { t } from "../shared/i18n";
import { toast } from "../shared/toast";
import type { Snapshot } from "../shared/types";

/** What this part needs of the page around it. */
export interface PairingHost {
  snapshot(): Snapshot | null;
}

let host: PairingHost;
let openDialog: (note?: string) => void = () => {};

/** Opens the pairing dialog, optionally with an explanation on top. */
export function openPairDialog(note?: string): void {
  openDialog(note);
}

/** Lists the pairing services that were found, to choose one from. */
export function renderPairFound(): void {
  const box = $("pair-found");
  const list = host.snapshot()?.pairing ?? [];
  if (list.length === 0) {
    box.innerHTML = "";
    return;
  }
  box.innerHTML =
    `<p>${esc(t("pair.found"))}</p>` +
    list
      .map((p) => `<button type="button" data-address="${esc(p.address)}">${esc(p.address)}</button>`)
      .join("");
}

export function setupPairDialog(next: PairingHost): void {
  host = next;
  const dlg = $<HTMLDialogElement>("dlg-pair");
  const address = $<HTMLInputElement>("pair-address");
  const code = $<HTMLInputElement>("pair-code");
  const error = $("pair-error");
  const submit = $<HTMLButtonElement>("pair-submit");

  const note = $("pair-note");
  openDialog = (text?: string) => {
    error.textContent = "";
    code.value = "";
    note.textContent = text ?? "";
    note.hidden = !text;
    renderPairFound();
    if (!dlg.open) dlg.showModal();
    address.focus();
  };
  $("btn-pair").addEventListener("click", () => openDialog());
  $("pair-cancel").addEventListener("click", () => dlg.close());
  $("pair-found").addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-address]");
    if (b) {
      address.value = b.dataset.address ?? "";
      code.focus();
    }
  });
  $("form-pair").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    error.textContent = "";
    submit.disabled = true;
    submit.textContent = t("pair.working");
    try {
      await invoke("pair_device", { address: address.value.trim(), code: code.value.trim() });
      dlg.close();
      toast("info", t("pair.success"));
    } catch (e) {
      const { text, detail } = describeError(e);
      error.textContent = detail ? `${text}\n${detail}` : text;
    } finally {
      submit.disabled = false;
      submit.textContent = t("pair.submit");
    }
  });
}
