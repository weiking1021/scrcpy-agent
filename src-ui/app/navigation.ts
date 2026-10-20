// The pages of the main window and how one goes between them: the sidebar (pages, the debug console, which is a
// window of its own, and folding the sidebar to icons) and the texts of the page that is shown.

import { invoke } from "@tauri-apps/api/core";
import { fillSettings } from "../settings/page";
import { $ } from "../shared/dom";
import { t, translateDom } from "../shared/i18n";
import { showError } from "../shared/toast";
import { setFolded } from "./sidebar";

const PAGES = ["devices", "files", "settings"] as const;
type Page = (typeof PAGES)[number];

function isPage(value: string | undefined): value is Page {
  return (PAGES as readonly (string | undefined)[]).includes(value);
}

let page: Page = "devices";

/** Writes the texts of the page in the language that is set, and the title of the page shown. */
export function translateAll(): void {
  translateDom();
  $("page-title").textContent = t(`nav.${page}`);
  if (page === "settings") fillSettings();
}

/** Shows one page of the window: the sidebar says which, and the pair button only belongs to the devices. */
function showPage(next: Page): void {
  page = next;
  for (const name of PAGES) $(`page-${name}`).hidden = name !== next;
  document.querySelectorAll<HTMLElement>(".nav-item[data-page]").forEach((b) => {
    if (b.dataset.page === next) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  });
  $("page-title").textContent = t(`nav.${next}`);
  $("btn-pair").hidden = next !== "devices";
  if (next === "settings") fillSettings();
}

/** The sidebar: pages, the debug console (a window of its own), and folding it to icons. */
export function setupNavigation(): void {
  document.querySelectorAll<HTMLButtonElement>(".nav-item[data-page]").forEach((b) => {
    b.addEventListener("click", () => {
      if (isPage(b.dataset.page)) showPage(b.dataset.page);
    });
  });
  $("nav-console").addEventListener("click", () => {
    invoke("open_console_window").catch(showError);
  });
  const sidebar = $("sidebar");
  const key = "sidebar-collapsed";
  try {
    if (localStorage.getItem(key) === "1") setFolded(sidebar, true, false);
  } catch {
    // Without storage the sidebar starts open.
  }
  $("nav-toggle").addEventListener("click", () => {
    const collapsed = !sidebar.classList.contains("collapsed");
    setFolded(sidebar, collapsed);
    try {
      localStorage.setItem(key, collapsed ? "1" : "0");
    } catch {
      // It is only a convenience.
    }
  });
  showPage(page);
}
