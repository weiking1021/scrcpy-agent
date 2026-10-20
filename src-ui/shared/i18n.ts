import { invoke } from "@tauri-apps/api/core";
import en from "./locales/en.json";
import zhTW from "./locales/zh-TW.json";

type Pack = Record<string, string>;

// Built-in packs. Users can add more by dropping `<code>.json` files into the
// `locales` folder of the data directory; a pack with the same code replaces
// the built-in one. Missing keys fall back to English.
const packs: Record<string, Pack> = { en, "zh-TW": zhTW };
let active = "en";

export async function loadUserPacks(): Promise<void> {
  try {
    const list = await invoke<{ code: string; content: string }[]>("list_user_locales");
    for (const item of list) {
      try {
        const parsed: unknown = JSON.parse(item.content);
        if (parsed && typeof parsed === "object") {
          packs[item.code] = Object.fromEntries(
            Object.entries(parsed).filter(([, v]) => typeof v === "string"),
          ) as Pack;
        }
      } catch {
        // A broken language file must never break the app.
      }
    }
  } catch {
    // Not running inside Tauri (plain browser preview).
  }
}

export function languages(): { code: string; name: string }[] {
  return Object.entries(packs).map(([code, p]) => ({ code, name: p._name ?? code }));
}

/** Picks the best pack for a saved preference or the system language. */
function resolve(pref: string | null): string {
  const wanted = pref ?? navigator.language;
  if (packs[wanted]) return wanted;
  const lower = wanted.toLowerCase();
  const exact = Object.keys(packs).find((c) => c.toLowerCase() === lower);
  if (exact) return exact;
  const base = lower.split("-")[0];
  if (base === "zh") return packs["zh-TW"] ? "zh-TW" : "en";
  return Object.keys(packs).find((c) => c.toLowerCase().split("-")[0] === base) ?? "en";
}

export function setLanguage(pref: string | null): void {
  active = resolve(pref);
  document.documentElement.lang = active;
}

/** What the program calls itself now: every text may say `{app}`, and gets this. */
let appName = "ScrcpyAgent";

/**
 * Says what the program is called: the name the user gave it (the setting), else the standard name of
 * the language (the text `app.name`). The texts that say `{app}` have to be written again after it.
 */
export function setAppName(setting: string | null): string {
  appName = setting ?? packs[active]?.["app.name"] ?? packs.en["app.name"];
  return appName;
}

export function t(key: string, vars: Record<string, string | number> = {}): string {
  const text = packs[active]?.[key] ?? packs.en[key] ?? key;
  const all: Record<string, string | number> = { app: appName, ...vars };
  return text.replace(/\{(\w+)\}/g, (_, name: string) => String(all[name] ?? `{${name}}`));
}

/** Fills `data-i18n`, `data-i18n-placeholder` and `data-i18n-title` attributes. */
export function translateDom(root: ParentNode = document): void {
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n!);
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]").forEach((el) => {
    el.setAttribute("placeholder", t(el.dataset.i18nPlaceholder!));
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    el.setAttribute("title", t(el.dataset.i18nTitle!));
  });
}
