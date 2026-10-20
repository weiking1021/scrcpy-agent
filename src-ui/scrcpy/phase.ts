// What is said in the sidebar of the release of scrcpy: that it is being looked for, that one has to be
// downloaded (with the button that asks), how far a download is, or which release is ready.

import { $, esc } from "../shared/dom";
import { t } from "../shared/i18n";
import type { Snapshot } from "../shared/types";
import { askAbout, showProgress } from "./scrcpy";

export function renderScrcpyStatus(s: Snapshot): void {
  const el = $("status");
  const st = s.scrcpy;
  switch (st.phase) {
    case "checking":
      el.innerHTML = esc(t("status.checking"));
      break;
    case "needs_download": {
      el.innerHTML =
        `${esc(t("status.needs_download", { version: st.version }))} ` +
        `<button type="button" class="link" id="status-download">${esc(t("status.download_button"))}</button>`;
      $("status-download").addEventListener("click", () => askAbout(st.version));
      break;
    }
    case "downloading": {
      const percent = Math.round(st.progress * 100);
      el.innerHTML =
        `${esc(t("status.downloading", { version: st.version, percent }))}` +
        `<progress max="1" value="${st.progress}"></progress>`;
      break;
    }
    case "ready":
      el.textContent = t("status.ready", { version: st.version });
      break;
  }
  showProgress(st.phase === "downloading" ? st.progress : null);
  el.dataset.phase = st.phase;
}
