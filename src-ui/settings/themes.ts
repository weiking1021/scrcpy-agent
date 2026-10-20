// The list of themes in the settings: the ones the core knows, in groups by whose they are.

import { invoke } from "@tauri-apps/api/core";
import { esc } from "../shared/dom";
import { t } from "../shared/i18n";
import type { ThemeInfo, ThemeList } from "../shared/types";

/** The themes there are, as the settings window lists them. */
let themeList: ThemeInfo[] = [];

/** The name a theme is listed by: the program's own are named in the language of the window. */
function themeLabel(theme: ThemeInfo): string {
  if (theme.source !== "built_in") return theme.name;
  const key = `theme.builtin.${theme.id}`;
  const text = t(key);
  return text === key ? theme.name : text;
}

/** The themes in groups, so that two with the same name are told apart by whose they are: the program's, the user's, and one group for each add-on. */
function themeGroups(): { label: string; themes: ThemeInfo[] }[] {
  const groups: { label: string; themes: ThemeInfo[] }[] = [];
  const add = (label: string, theme: ThemeInfo) => {
    const group = groups.find((g) => g.label === label);
    if (group) group.themes.push(theme);
    else groups.push({ label, themes: [theme] });
  };
  for (const theme of themeList) {
    if (theme.source === "built_in") add(t("settings.theme_group_builtin"), theme);
    else if (theme.source === "user") add(t("settings.theme_group_user"), theme);
    else add(theme.id.split("/")[0], theme);
  }
  return groups;
}

/** Fills the list of themes and chooses `chosen`. */
export function renderThemeOptions(select: HTMLSelectElement, chosen: string): void {
  const option = (value: string, text: string) => `<option value="${esc(value)}">${esc(text)}</option>`;
  const html = themeGroups().map(
    (g) => `<optgroup label="${esc(g.label)}">${g.themes.map((th) => option(th.id, themeLabel(th))).join("")}</optgroup>`,
  );
  // A theme that is chosen but not there (an add-on that is not loaded) stays chosen: it comes back with its add-on.
  if (!themeList.some((theme) => theme.id === chosen)) {
    html.push(option(chosen, t("settings.theme_missing", { id: chosen })));
  }
  select.innerHTML = html.join("");
  select.value = chosen;
}

/** Asks the core for the themes there are now. */
export async function loadThemes(): Promise<void> {
  try {
    themeList = (await invoke<ThemeList>("list_themes")).themes;
  } catch {
    // Not running inside Tauri (plain browser preview).
  }
}
