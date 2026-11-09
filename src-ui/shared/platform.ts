// Host differences only; every platform uses the same pages and settings.
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

export async function initPlatform(): Promise<void> {
  const platform = await invoke<string>("host_platform");
  document.documentElement.dataset.platform = platform;
  if (platform !== "android") return;
  // Desktop process/window controls have no Android implementation in the viewer.
  for (const id of ["set-close-to-tray", "set-adb-backend", "set-scrcpy-mode", "set-keyboard-mode",
    "set-toolbar-enabled", "set-toolbar-side", "set-toolbar-on-top", "set-toolbar-buttons", "ds-keyboard",
    "set-extra-args"]) {
    const row = document.getElementById(id)?.closest<HTMLElement>(".setting");
    if (row) row.hidden = true;
  }
  // Native folder installation is unavailable on Android.
  document.getElementById("addon-add")?.closest<HTMLElement>(".group")?.setAttribute("hidden", "");
  const emptyFiles = document.getElementById("files-empty");
  if (emptyFiles) emptyFiles.dataset.i18n = "files.empty_mobile";
  document.querySelectorAll<HTMLElement>(".group").forEach((group) => {
    const rows = [...group.querySelectorAll<HTMLElement>(".setting")];
    if (rows.length > 0 && rows.every((row) => row.hidden)) group.hidden = true;
  });
  if (!document.body.classList.contains("app")) {
    const back = document.createElement("a");
    back.className = "mobile-back";
    back.href = "/index.html";
    back.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14.5 6l-6 6 6 6"/></svg>';
    const label = document.createElement("span");
    label.dataset.i18n = "nav.devices";
    label.textContent = t("nav.devices");
    back.append(label);
    document.body.prepend(back);
  }
}
