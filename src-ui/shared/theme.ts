// The look of a window: the theme the settings choose is a stylesheet put on top of the
// program's own (`styles.css`). Every window calls `initTheme` once; a change of the setting
// reaches all of them as the state event, and each puts the new theme on its page at once.
//
// The stylesheet is loaded from the address `theme://` (see `commands/themes.rs`), not put into
// the page as text: what the stylesheet asks for with a relative path (a picture, a font of the
// theme's own folder) is then found next to it.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppliedTheme, CoreEvent } from "./types";

const LINK_ID = "theme-css";

/** How many times a stylesheet was asked for: part of its address, so one that was edited is read again. */
let asked = 0;

/** The first part of the address of a theme's files: its id, with `~` for `/` (as `ThemeUrl::segment` in the core). */
function segment(id: string): string {
  return id.replace("/", "~");
}

/** Where the files of themes are, as this system writes such an address. */
function base(): string {
  // An empty path gives the address of the root, in the form Tauri uses on this system.
  return convertFileSrc("", "theme");
}

/** Puts a stylesheet on the page; the one that was there goes when the new one has loaded (nothing flashes). */
function putStylesheet(href: string): void {
  const previous = document.getElementById(LINK_ID);
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = href;
  link.addEventListener("load", () => {
    if (previous?.parentNode) previous.remove();
    link.id = LINK_ID;
  });
  // A stylesheet that could not be loaded: the page keeps the one it had.
  link.addEventListener("error", () => link.remove());
  document.head.appendChild(link);
}

/** Puts a theme on the page: what it is made for, and its stylesheet. */
export function applyTheme(theme: AppliedTheme): void {
  const root = document.documentElement;
  if (theme.base === "auto") delete root.dataset.base;
  else root.dataset.base = theme.base;
  asked += 1;
  putStylesheet(`${base()}${segment(theme.id)}/theme.css?v=${asked}`);
}

/** Puts the chosen theme on the page, and again whenever the setting changes. */
export async function initTheme(): Promise<void> {
  let chosen: string | null = null;
  const show = async () => {
    try {
      applyTheme(await invoke<AppliedTheme>("current_theme"));
    } catch {
      // Not running inside Tauri (plain browser preview): the page keeps the program's own look.
    }
  };
  await show();
  try {
    await listen<CoreEvent>("core-event", (event) => {
      if (event.payload.kind !== "state") return;
      const theme = event.payload.data.settings.theme;
      if (chosen !== null && theme !== chosen) void show();
      chosen = theme;
    });
  } catch {
    // Not running inside Tauri.
  }
}
