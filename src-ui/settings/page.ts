// The settings page: the controls show what is set, and a change is kept as soon as it is made. The core decides
// what a setting may be (a value it refuses is put back); this reads the controls, asks it to keep them, and shows
// what it kept.

import { invoke } from "@tauri-apps/api/core";
import { $, esc } from "../shared/dom";
import { languages, t } from "../shared/i18n";
import { wholeNumber } from "../shared/numbers";
import { showError, toast } from "../shared/toast";
import { isAdbBackend, isKeyboardMode, isScrcpyMode, isSide } from "../shared/types";
import type { FileManager, Settings, Snapshot } from "../shared/types";
import { parseAppName } from "./appName";
import { loadThemes, renderThemeOptions } from "./themes";
import { layoutPrefs, loadToolbarLayout, setupToolbarLayout } from "./toolbarLayout";

/** What this part needs of the page around it. */
export interface SettingsHost {
  snapshot(): Snapshot | null;
  /** The settings were kept: the rest of the window follows them (the language, the name, the list). */
  saved(settings: Settings): void;
  /** The page is filled: the parts of it that other features own (the release of scrcpy, the add-ons) read theirs again. */
  filled(): void;
}

let host: SettingsHost;

/** What this system can do, asked once at the start: it decides which rows of the page are shown. */
let explorerAvailable = false;
let launchAvailable = false;
/** The file managers of this system that the menu can be written for (none on Windows). */
let detectedManagers: FileManager[] = [];

export async function loadSystemFeatures(): Promise<void> {
  explorerAvailable = await invoke<boolean>("explorer_menu_available").catch(() => false);
  launchAvailable = await invoke<boolean>("launch_at_login_available").catch(() => false);
  detectedManagers = await invoke<FileManager[]>("detected_file_managers").catch(() => []);
}

/** The devices that can be the default one: every device of the list, and "none". */
function fillDefaultDevices(select: HTMLSelectElement, current: string | null, snap: Snapshot): void {
  const devices = snap.devices;
  const options = [`<option value="">${esc(t("settings.default_device_none"))}</option>`];
  for (const d of devices) {
    options.push(`<option value="${esc(d.id)}">${esc(d.label ?? d.name)} (${esc(d.id)})</option>`);
  }
  // A default device that is not in the list now is still the one that was chosen.
  if (current !== null && !devices.some((d) => d.id === current)) {
    options.push(`<option value="${esc(current)}">${esc(current)}</option>`);
  }
  select.innerHTML = options.join("");
  select.value = current ?? "";
}

/** The controls of the page. */
function controls() {
  return {
    lang: $<HTMLSelectElement>("set-language"),
    theme: $<HTMLSelectElement>("set-theme"),
    appName: $<HTMLInputElement>("set-app-name"),
    closeToTray: $<HTMLInputElement>("set-close-to-tray"),
    launchAtLogin: $<HTMLInputElement>("set-launch-at-login"),
    launchRow: $("set-launch-row"),
    extra: $<HTMLTextAreaElement>("set-extra-args"),
    toolbarEnabled: $<HTMLInputElement>("set-toolbar-enabled"),
    toolbarSide: $<HTMLSelectElement>("set-toolbar-side"),
    toolbarOnTop: $<HTMLInputElement>("set-toolbar-on-top"),
    adbBackend: $<HTMLSelectElement>("set-adb-backend"),
    scrcpyMode: $<HTMLSelectElement>("set-scrcpy-mode"),
    keyboardMode: $<HTMLSelectElement>("set-keyboard-mode"),
    bitRate: $<HTMLInputElement>("set-bit-rate"),
    refresh: $<HTMLInputElement>("set-device-refresh"),
    requestClose: $<HTMLInputElement>("set-request-close"),
    sendDirectory: $<HTMLInputElement>("set-send-directory"),
    defaultDevice: $<HTMLSelectElement>("set-default-device"),
    explorerMenu: $<HTMLInputElement>("set-explorer-menu"),
    explorerRow: $("set-explorer-row"),
    explorerHint: $("set-explorer-hint"),
    managersRow: $("set-fm-row"),
    managersList: $("set-fm-list"),
  };
}

type Controls = ReturnType<typeof controls>;

/** The ticked file managers of the list; the ones that are not found keep what was set. */
function disabledManagers(c: Controls, snap: Snapshot): FileManager[] {
  const unchecked = detectedManagers.filter(
    (m) => !c.managersList.querySelector<HTMLInputElement>(`input[data-manager="${m}"]`)?.checked,
  );
  const absent = snap.settings.disabled_file_managers.filter((m) => !detectedManagers.includes(m));
  return [...absent, ...unchecked];
}

/** Puts what is set into the controls (when the page is shown, and after a save that failed). */
export function fillSettings(): void {
  const snap = host.snapshot();
  if (!snap) return;
  const c = controls();
  const s = snap.settings;
  c.adbBackend.value = s.adb_backend;
  c.scrcpyMode.value = s.scrcpy_mode;
  c.keyboardMode.value = s.keyboard_mode;
  c.bitRate.value = String(s.video_bit_rate_mbps);
  c.refresh.value = String(s.device_refresh_seconds);
  c.requestClose.value = String(s.request_close_seconds);
  c.sendDirectory.value = s.send_directory;
  fillDefaultDevices(c.defaultDevice, s.default_device, snap);
  c.explorerMenu.checked = s.explorer_menu;
  c.explorerRow.hidden = !explorerAvailable;
  // On Linux the menu is written for each file manager found, and each can be left out.
  c.managersRow.hidden = !explorerAvailable || detectedManagers.length === 0;
  c.explorerHint.textContent = t(
    detectedManagers.length === 0 ? "settings.explorer_menu_hint" : "settings.explorer_menu_hint_linux",
  );
  c.managersList.innerHTML = detectedManagers
    .map(
      (m) =>
        `<label class="check"><input type="checkbox" data-manager="${m}"${s.disabled_file_managers.includes(m) ? "" : " checked"} />` +
        `<span>${esc(t(`file_manager.${m}`))}</span></label>`,
    )
    .join("");
  c.toolbarEnabled.checked = s.toolbar.enabled;
  c.toolbarSide.value = s.toolbar.side;
  c.toolbarOnTop.checked = s.toolbar.always_on_top;
  void loadToolbarLayout();
  c.lang.innerHTML =
    `<option value="">${esc(t("settings.language_auto"))}</option>` +
    languages()
      .map((l) => `<option value="${esc(l.code)}">${esc(l.name)}</option>`)
      .join("");
  c.lang.value = s.language ?? "";
  c.appName.value = s.app_name ?? "";
  // The list is asked for each time (a theme may have been put into the folder, or an add-on may have one).
  void loadThemes().then(() => {
    const now = host.snapshot();
    if (now) renderThemeOptions(c.theme, now.settings.theme);
  });
  c.closeToTray.checked = s.close_to_tray;
  c.launchRow.hidden = !launchAvailable;
  c.launchAtLogin.checked = s.launch_at_login;
  c.extra.value = s.scrcpy_extra_args.join("\n");
  $("set-data-dir").textContent = snap.data_dir;
  host.filled();
}

/**
 * The settings the controls say, built from what is set now wherever a control holds something that cannot be
 * a value (so a mistake of the page never changes a setting). `null` when the name of the program typed is not a name.
 */
function readControls(c: Controls, snap: Snapshot, resetLayout: boolean): Settings | null {
  const now = snap.settings;
  // An empty box is the standard name; a text that cannot be a name is not saved.
  const typedName = c.appName.value.trim() === "" ? null : parseAppName(c.appName.value);
  if (c.appName.value.trim() !== "" && typedName === null) return null;
  return {
    toolbar: {
      enabled: c.toolbarEnabled.checked,
      side: isSide(c.toolbarSide.value) ? c.toolbarSide.value : now.toolbar.side,
      always_on_top: c.toolbarOnTop.checked,
      // An empty layout is the default one (every button shown, in the order of the toolbar).
      buttons: resetLayout ? [] : layoutPrefs(),
    },
    adb_backend: isAdbBackend(c.adbBackend.value) ? c.adbBackend.value : now.adb_backend,
    scrcpy_mode: isScrcpyMode(c.scrcpyMode.value) ? c.scrcpyMode.value : now.scrcpy_mode,
    keyboard_mode: isKeyboardMode(c.keyboardMode.value) ? c.keyboardMode.value : now.keyboard_mode,
    video_bit_rate_mbps: wholeNumber(c.bitRate.value, 0, 200, now.video_bit_rate_mbps),
    device_refresh_seconds: wholeNumber(c.refresh.value, 0, 3600, now.device_refresh_seconds),
    request_close_seconds: wholeNumber(c.requestClose.value, 0, 60, now.request_close_seconds),
    send_directory: c.sendDirectory.value.trim(),
    default_device: c.defaultDevice.value === "" ? null : c.defaultDevice.value,
    explorer_menu: explorerAvailable ? c.explorerMenu.checked : now.explorer_menu,
    disabled_file_managers: disabledManagers(c, snap),
    language: c.lang.value || null,
    app_name: typedName,
    theme: c.theme.value || now.theme,
    // The choice of the release is made (and kept) by the core when it is made, not with the other settings.
    scrcpy_version: now.scrcpy_version,
    close_to_tray: c.closeToTray.checked,
    launch_at_login: launchAvailable ? c.launchAtLogin.checked : now.launch_at_login,
    scrcpy_extra_args: c.extra.value
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean),
  };
}

let savedTimer: number | undefined;

/** Reads the controls and keeps what they say at once; a value that is refused puts the controls back. */
async function save(resetLayout = false): Promise<void> {
  const snap = host.snapshot();
  if (!snap) return;
  const settings = readControls(controls(), snap, resetLayout);
  if (settings === null) {
    toast("error", t("settings.app_name_invalid"));
    fillSettings();
    return;
  }
  const state = $("settings-state");
  try {
    await invoke("save_settings", { settings });
    // What was saved is what the controls show from now on, without waiting for the state event
    // (the controls are filled again, and would show the old value until it comes).
    host.saved(settings);
    // The list is read again: after a reset it is the default one, and the core may know buttons the page did not.
    void loadToolbarLayout();
    state.textContent = t("settings.saved");
    state.classList.add("shown");
    window.clearTimeout(savedTimer);
    savedTimer = window.setTimeout(() => state.classList.remove("shown"), 2000);
  } catch (e) {
    showError(e);
    fillSettings();
  }
}

export function setupSettingsPage(next: SettingsHost): void {
  host = next;
  setupToolbarLayout({ changed: () => void save(), reset: () => void save(true) });
  $("set-open-locales").addEventListener("click", () => {
    invoke("reveal_data_dir").catch(showError);
  });
  $("set-open-themes").addEventListener("click", () => {
    invoke("reveal_themes_dir").catch(showError);
  });
  // A choice or a switch is kept when it is changed; a number or a text when it is
  // left (so half a number is not saved).
  $("page-settings").addEventListener("change", (ev) => {
    const target = ev.target as HTMLElement;
    // (What has an action of its own, like the choice of the release of scrcpy, is not a setting that is saved here.)
    if (target.matches("select, input, textarea") && target.dataset.action === undefined) void save();
  });
}
