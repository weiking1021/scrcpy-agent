export type DeviceState = "online" | "unauthorized" | "offline" | "discovered" | "disconnected";

export interface Device {
  id: string;
  name: string;
  label: string | null;
  state: DeviceState;
  transport: "usb" | "wifi" | null;
  adb_serial: string | null;
  address: string | null;
  android: string | null;
  sdk: number | null;
  known: boolean;
}

export type ScrcpyStatus =
  | { phase: "checking" }
  | { phase: "needs_download"; version: string }
  | { phase: "downloading"; version: string; progress: number }
  | { phase: "ready"; version: string };

/** Why a release of scrcpy could not be listed, offered or downloaded (the same names as the core's). */
export const RELEASE_FAILURES = ["unreachable", "not_found", "unsupported", "checksum_mismatch", "unpack", "storage"] as const;
export const OFFER_ERRORS = ["bad_release", "no_asset", "insecure", "no_checksum"] as const;
export type ReleaseFailure =
  | (typeof RELEASE_FAILURES)[number]
  | { offer: (typeof OFFER_ERRORS)[number] };

/** What is to be downloaded of a release, as GitHub tells it. */
export interface Offer {
  tag: string;
  asset: string;
  url: string;
  size: number | null;
  sha256: string;
}

/** The releases of scrcpy that are installed, the one in use and the default. */
export interface ScrcpyVersions {
  default: string;
  current: string | null;
  /** The release that is wanted (it may not be installed). */
  wanted: string;
  installed: string[];
  /** The folder the releases are installed in. */
  folder: string;
}

export const SIDES = ["left", "right", "top", "bottom"] as const;
export type Side = (typeof SIDES)[number];

export function isSide(value: string): value is Side {
  return (SIDES as readonly string[]).includes(value);
}

/** The floating toolbar next to the scrcpy window. */
export interface ToolbarSettings {
  enabled: boolean;
  side: Side;
  always_on_top: boolean;
  /** The buttons the user arranged: the order, and which are hidden. One that is not listed comes after the others. */
  buttons: ButtonPref[];
}

/** One entry of the layout of the toolbar. */
export interface ButtonPref {
  id: ButtonId;
  visible: boolean;
}

/** One row of the list of the toolbar's buttons in the settings (every button there is, shown or not). */
export interface LayoutEntry {
  id: ButtonId;
  /** The text an add-on gave its button; null for a built-in one. */
  label: string | null;
  visible: boolean;
}

export type BuiltInButton =
  | "back"
  | "home"
  | "app_switch"
  | "power"
  | "volume_up"
  | "volume_down"
  | "rotate"
  | "notifications"
  | "send_file"
  | "screenshot";

/** A built-in button is its name; a button of an add-on carries the add-on's id. */
export type ButtonId = BuiltInButton | { addon: { addon: string; id: string } };

/** Why a button cannot be pressed: it has no action yet, or it needs the proxy. */
export type Unavailable = "not_yet" | "needs_proxy";

export interface ToolbarButton {
  id: ButtonId;
  /** The text an add-on gave its button; null for the built-in buttons (their text is in the language pack). */
  label: string | null;
  /** False for a button that cannot be pressed now; it is shown greyed out. */
  enabled: boolean;
  unavailable: Unavailable | null;
}

/** How the application talks to adb. */
export const ADB_BACKENDS = ["server", "command_line"] as const;
export type AdbBackend = (typeof ADB_BACKENDS)[number];

export function isAdbBackend(value: string): value is AdbBackend {
  return (ADB_BACKENDS as readonly string[]).includes(value);
}

/** Whether scrcpy runs through the proxy that add-ons' special features need. */
export const SCRCPY_MODES = ["official", "proxy"] as const;
export type ScrcpyMode = (typeof SCRCPY_MODES)[number];

export function isScrcpyMode(value: string): value is ScrcpyMode {
  return (SCRCPY_MODES as readonly string[]).includes(value);
}

/** How the computer's keyboard is sent to the device: through Android's API, or as a physical keyboard. */
export const KEYBOARD_MODES = ["sdk", "uhid"] as const;
export type KeyboardMode = (typeof KEYBOARD_MODES)[number];

export function isKeyboardMode(value: string): value is KeyboardMode {
  return (KEYBOARD_MODES as readonly string[]).includes(value);
}

export interface Settings {
  toolbar: ToolbarSettings;
  adb_backend: AdbBackend;
  scrcpy_mode: ScrcpyMode;
  keyboard_mode: KeyboardMode;
  /** The bit rate of the video in megabits per second: 0 (scrcpy's own choice) or 1 to 200. */
  video_bit_rate_mbps: number;
  /** How often a device's window reads the device again: 0 (never) or 1 to 3600 seconds. */
  device_refresh_seconds: number;
  /** Seconds the small window of a request stays after it went well: 0 to 60. */
  request_close_seconds: number;
  language: string | null;
  /** What the user calls the program; null is the standard name of the language. */
  app_name: string | null;
  /** The id of the theme the windows are drawn in (a theme that is not there is the default one). */
  theme: string;
  /** The release of scrcpy the user chose; null is the one the program is checked against. */
  scrcpy_version: string | null;
  /** Closing the main window hides it in the system tray instead of quitting. */
  close_to_tray: boolean;
  /** The program starts when the user logs in, in the tray. */
  launch_at_login: boolean;
  /** The folder on a device that files are sent to. */
  send_directory: string;
  /** The device used for "the default device" (its id), or null. */
  default_device: string | null;
  /** The entries of the Explorer menu are registered. */
  explorer_menu: boolean;
  /** The file managers of Linux the menu is not written for, though they are there. */
  disabled_file_managers: FileManager[];
  scrcpy_extra_args: string[];
}

/** What a theme is made for: the colour scheme of the parts the system draws. */
export type ThemeBase = "auto" | "light" | "dark";

/** What the settings window lists of a theme. */
export interface ThemeInfo {
  id: string;
  name: string;
  base: ThemeBase;
  source: "built_in" | "user" | "addon";
}

/** A folder of the user's themes that is not used, and why. */
export interface ThemeProblem {
  folder: string;
  kind: string;
  error?: string;
}

export interface ThemeList {
  themes: ThemeInfo[];
  problems: ThemeProblem[];
}

/** The theme a window puts on its page. */
export interface AppliedTheme {
  id: string;
  base: ThemeBase;
  css: string;
}

/** A file manager of Linux that has a right-click menu this program can write. */
export type FileManager = "dolphin" | "nemo" | "nautilus";

export interface PairingService {
  name: string;
  address: string;
}

export interface Snapshot {
  scrcpy: ScrcpyStatus;
  /** The release that is wanted while it is not installed: the user is asked whether to download it. */
  scrcpy_wanted: string | null;
  devices: Device[];
  pairing: PairingService[];
  sessions: string[];
  settings: Settings;
  data_dir: string;
}

export const NOTICE_CODES = [
  "audio_unavailable",
  "session_failed",
  "adb_start_failed",
  "mdns_unavailable",
  "proxy_unavailable",
  "options_unavailable",
  "device_not_online",
] as const;

export type NoticeCode = (typeof NOTICE_CODES)[number];

/** A transient message from the core. `detail` is raw text, shown as is. */
export interface Notice {
  level: "error" | "info";
  code: NoticeCode;
  detail: string;
  device: string | null;
}

/** The typed error every command rejects with; switch on `code`. */
export type CoreError =
  | { code: "not_ready" }
  | { code: "invalid_address" }
  | { code: "invalid_pairing_code" }
  | { code: "not_paired" }
  | { code: "unreachable" }
  | { code: "connect_in_progress" }
  | { code: "unknown_device" }
  | { code: "not_online" }
  | { code: "already_open" }
  | { code: "not_available" }
  | { code: "pair_not_confirmed"; detail: string }
  | { code: "no_proxy" }
  | { code: "control_unavailable" }
  | { code: "invalid_package" }
  | { code: "unsupported_scrcpy" }
  | { code: "app_list_failed"; detail: string }
  | { code: "invalid_label" }
  | { code: "invalid_directory" }
  | { code: "not_a_file" }
  | { code: "bad_file_name" }
  | { code: "no_app" }
  | { code: "explorer_menu"; detail: string }
  | { code: "autostart"; detail: string }
  | { code: "addon"; detail: InstallErrorDetail }
  | { code: "unknown_setting" }
  | { code: "invalid_setting"; detail: SettingProblem }
  | { code: "scrcpy_release"; detail: ReleaseFailure }
  | { code: "scrcpy_not_installed" }
  | { code: "screens_open" }
  | { code: "adb_not_started"; detail: string }
  | { code: "shortcut"; detail: string }
  | { code: "storage"; detail: string }
  | { code: "unavailable" }
  | { code: "command_failed"; detail: number }
  | { code: "other"; detail: string };

export const CORE_ERROR_CODES = [
  "not_ready",
  "invalid_address",
  "invalid_pairing_code",
  "not_paired",
  "unreachable",
  "connect_in_progress",
  "unknown_device",
  "not_online",
  "already_open",
  "not_available",
  "pair_not_confirmed",
  "no_proxy",
  "control_unavailable",
  "invalid_package",
  "unsupported_scrcpy",
  "app_list_failed",
  "invalid_label",
  "invalid_directory",
  "not_a_file",
  "bad_file_name",
  "no_app",
  "explorer_menu",
  "autostart",
  "addon",
  "unknown_setting",
  "invalid_setting",
  "scrcpy_release",
  "scrcpy_not_installed",
  "screens_open",
  "adb_not_started",
  "shortcut",
  "storage",
  "unavailable",
  "command_failed",
  "other",
] as const satisfies readonly CoreError["code"][];

// ---------------------------------------------------------------- add-ons

/** What an add-on may ask to be allowed (the same names as `permission.rs`). */
export const ADDON_PERMISSIONS = ["device_shell", "run_process", "screen_capture"] as const;
export type AddonPermission = (typeof ADDON_PERMISSIONS)[number];

export const MANIFEST_ERRORS = [
  "not_json",
  "not_an_object",
  "bad_id",
  "bad_name",
  "bad_version",
  "bad_permissions",
  "bad_platforms",
  "bad_api",
  "bad_settings",
] as const;
export type ManifestError = (typeof MANIFEST_ERRORS)[number];

/** What an add-on lets the user set (written from the Rust types, `addon-api/generated`). */
export type SettingKind =
  | { type: "boolean"; default: boolean }
  | { type: "number"; default: number; min: number; max: number }
  | { type: "text"; default: string; max_length: number }
  | { type: "choice"; default: string; options: { value: string; label: string }[] };
export type SettingSpec = { id: string; label: string; hint: string | null } & SettingKind;
export type SettingValue = boolean | number | string;

/** The settings of an add-on: what it declared, and the value of each now. */
export interface AddonSettingsView {
  specs: SettingSpec[];
  values: Record<string, SettingValue>;
}

/** Why a value was refused for a setting. */
export const SETTING_PROBLEMS = ["wrong_type", "out_of_range", "too_long", "has_control", "not_an_option"] as const;
export type SettingProblem = (typeof SETTING_PROBLEMS)[number];

export const UNSUPPORTED_REASONS = ["api_too_new", "platform_not_listed"] as const;
export type Unsupported = (typeof UNSUPPORTED_REASONS)[number];

/** Why an add-on was not installed: a plain reason, or one that carries the reason inside it. */
export const INSTALL_ERRORS = [
  "not_absolute",
  "not_a_folder",
  "no_manifest",
  "manifest_too_large",
  "no_main_script",
  "already_installed",
  "not_installed",
  "storage",
] as const;
export type InstallErrorDetail =
  | (typeof INSTALL_ERRORS)[number]
  | { manifest: ManifestError }
  | { unsupported: Unsupported };

/** What an installed add-on is now. */
export const ADDON_STATUSES = [
  "ready",
  "folder_missing",
  "no_manifest",
  "id_changed",
  "no_main_script",
  "permissions_changed",
] as const;
export type AddonStatus =
  | (typeof ADDON_STATUSES)[number]
  | { manifest: ManifestError }
  | { unsupported: Unsupported };

export interface AddonManifest {
  id: string;
  name: string;
  version: string;
  platforms: string[];
  permissions: AddonPermission[];
  api: number;
  settings: SettingSpec[];
}

export interface AddonInfo {
  id: string;
  folder: string;
  manifest: AddonManifest | null;
  /** What the user gave that the manifest still asks for. */
  granted: AddonPermission[];
  status: AddonStatus;
  /** False when the user switched it off: it stays installed and does not run. */
  enabled: boolean;
}

/** Why an add-on did not start: a plain reason, or one with the text of the script or the system. */
export const START_FAILURES = ["timeout", "unreadable", "script", "thread"] as const;
export type StartFailure = "timeout" | "unreadable" | { script: string } | { thread: string };

export type RunState = "running" | "not_running" | "disabled" | { failed: StartFailure };

/** An installed add-on in the list: what it is, and what it is doing. */
export interface InstalledAddon extends AddonInfo {
  run: RunState;
  /** What it tried to use since it started without having the permission. */
  denied: AddonPermission[];
}

/** An app installed on a device, as the core lists it. */
export interface AppEntry {
  package: string;
  name: string;
  system: boolean;
}

export type ShortcutPlace = "desktop" | "start_menu";

/** What a shortcut is on this system: a Windows link, or a desktop entry of the Linux desktops. */
export type ShortcutKind = "windows_link" | "desktop_entry";

/** Which places have a shortcut of an app now. */
export interface ShortcutState {
  desktop: boolean;
  start_menu: boolean;
}

/** An entry of an app's menu: the built-in ones are plain names, an add-on's carries its owner. */
export type AppMenuItemId =
  | "open"
  | "open_on_main_screen"
  | "toggle_favorite"
  | "shortcut_desktop"
  | "shortcut_start_menu"
  | { addon: { addon: string; id: string } };

export interface AppMenuItem {
  id: AppMenuItemId;
  /** The text of an add-on's entry; a built-in entry has none (the UI has its own). */
  label: string | null;
  enabled: boolean;
}

/** An entry of a device card's menu. Built-in ones are plain names; add-on ones carry their owner. */
export type MenuItemId = "open_app" | { addon: { addon: string; id: string } };

export interface MenuItem {
  id: MenuItemId;
  /** The text of an add-on's entry; a built-in entry has none (the UI has its own). */
  label: string | null;
  enabled: boolean;
}

export type AppDisplay = "virtual_display" | "main_display";

/** The favourite apps of a device and the ones opened lately (package names; the newest recent one first). */
export interface AppUsage {
  favorites: string[];
  recent: string[];
}

/** The apps of a device as last listed; `updated_at` is seconds since 1970, 0 if never. */
export interface AppCache {
  updated_at: number;
  apps: AppEntry[];
}

export interface DeviceSystem {
  manufacturer: string | null;
  brand: string | null;
  model: string | null;
  device: string | null;
  soc: string | null;
  android: string | null;
  sdk: number | null;
  security_patch: string | null;
  build: string | null;
  abi: string | null;
  locale: string | null;
}

export type BatteryStatus = "charging" | "discharging" | "not_charging" | "full";

export interface Battery {
  level: number;
  scale: number;
  status: BatteryStatus | null;
  /** Tenths of a degree Celsius. */
  temperature: number | null;
  /** Millivolts. */
  voltage: number | null;
  ac: boolean | null;
  usb: boolean | null;
  wireless: boolean | null;
}

export interface DeviceInfo {
  system: DeviceSystem;
  battery: Battery | null;
  storage: { total_kb: number; used_kb: number; available_kb: number } | null;
  memory: { total_kb: number; available_kb: number } | null;
  uptime_seconds: number | null;
  screen: {
    width: number;
    height: number;
    physical_width: number;
    physical_height: number;
    density: number | null;
  } | null;
}

export interface DeviceDetails {
  device: Device;
  reported_name: string;
  first_seen: number;
  last_seen: number;
  info: DeviceInfo | null;
}

export function isCoreError(e: unknown): e is CoreError {
  return (
    typeof e === "object" &&
    e !== null &&
    "code" in e &&
    typeof e.code === "string" &&
    (CORE_ERROR_CODES as readonly string[]).includes(e.code)
  );
}

/** From the most important to the most detailed. */
export const LOG_LEVELS = ["error", "warn", "info", "debug"] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export function isLogLevel(value: string): value is LogLevel {
  return (LOG_LEVELS as readonly string[]).includes(value);
}

/** A line of the core's debug log. `code` says what it is about; `message` is for people. */
export interface LogLine {
  seq: number;
  at_ms: number;
  level: LogLevel;
  code: string;
  message: string;
}

export type TransferPhase = "sending" | "done" | "failed";

/** How far the sending of one file is. */
export interface TransferUpdate {
  id: number;
  device: string;
  /** The name the file has on the device. */
  name: string;
  sent: number;
  total: number;
  phase: TransferPhase;
  error: CoreError | null;
}

/** The end of opening a file on a device. */
export interface OpenedUpdate {
  device: string;
  file: string;
  remote: string | null;
  error: CoreError | null;
}

export type ShellAction = "send_default" | "send_choose" | "open_default" | "open_choose";

/** How one file of a request is doing. */
export type RequestItemState =
  | { state: "waiting" }
  | { state: "working" }
  | { state: "done" }
  | { state: "failed"; error: CoreError };

export type RequestItem = { file: string } & RequestItemState;

export const REQUEST_PHASES = ["choosing", "working", "done", "failed"] as const;
export type RequestPhase = (typeof REQUEST_PHASES)[number];

/** What the small window of a request from a file manager shows. */
export interface RequestView {
  /** Tells this request from an older one. */
  id: number;
  action: ShellAction;
  /** The device chosen (or the default one); null while it is asked for. */
  device: string | null;
  /** It was for the default device, but there is none that is ready. */
  no_default: boolean;
  items: RequestItem[];
  phase: RequestPhase;
  /** The window closes itself after this many milliseconds, if it should. */
  close_after_ms: number | null;
}

export interface SendOutcome {
  file: string;
  remote: string | null;
  error: CoreError | null;
}

export type OpenSupport =
  /** `type_known` is false when the kind of the file is not known: the count is of every app that can show a file. */
  | { kind: "apps"; count: number; is_default: boolean; type_known: boolean }
  | { kind: "no_app" }
  | { kind: "unreachable" };

export type CoreEvent =
  | { kind: "state"; data: Snapshot }
  | { kind: "notice"; data: Notice }
  | { kind: "log"; data: LogLine }
  | { kind: "device_info"; data: { device: string; info: DeviceInfo } }
  | { kind: "transfer"; data: TransferUpdate }
  | { kind: "opened"; data: OpenedUpdate }
  | { kind: "request"; data: RequestView | null };
