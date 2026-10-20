// The messages that appear for a moment in a corner of a window (the page has an element `toasts` for them).

import { $, esc } from "./dom";
import { describeError } from "./errors";

export type ToastLevel = "error" | "info";

/** Shows a message; a click takes it away, and it goes by itself (an error stays longer). */
export function toast(level: ToastLevel, text: string, detail = ""): void {
  const el = document.createElement("div");
  el.className = `toast ${level}`;
  el.innerHTML = `<strong>${esc(text)}</strong>${detail ? `<pre>${esc(detail)}</pre>` : ""}`;
  el.addEventListener("click", () => el.remove());
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), level === "error" ? 12000 : 6000);
}

/** Shows what went wrong, as the error says (`describeError`). */
export function showError(e: unknown): void {
  const { text, detail } = describeError(e);
  toast("error", text, detail);
}
