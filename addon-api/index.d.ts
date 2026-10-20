// Types for the add-ons of scrcpy-agent (the interface `api` 1): the global `host`, `console` and the timers a
// script gets. Install this package, and compile your script (`main.js`) with `"lib": ["ES2022"]` and no `"dom"`: a
// script runs in an engine with no browser and no Node, so those two sets of types would promise what is not there.
//
// What a type can say about data is written from the Rust types of the program (`./generated`, made by
// `npm run gen:addon-types`); only the shape of the functions is written here, and `check/host-members.ts` fails to
// compile if `Host` has other functions than the program lists.

import type { AddonEventMap } from "./generated/AddonEventMap";
import type { AddonPermission } from "./generated/AddonPermission";
import type { MenuRegisterCode } from "./generated/MenuRegisterCode";
import type { ProcessErrorCode } from "./generated/ProcessErrorCode";
import type { ScreenErrorCode } from "./generated/ScreenErrorCode";
import type { SettingValue } from "./generated/SettingValue";

export type { AddonEventMap, AddonPermission, MenuRegisterCode, ProcessErrorCode, ScreenErrorCode, SettingValue };
// What an author declares in `addon.json` under `settings` (a list of these).
export type { ChoiceOption } from "./generated/ChoiceOption";
export type { SettingKind } from "./generated/SettingKind";
export type { SettingSpec } from "./generated/SettingSpec";
export type { SettingChangedEventData } from "./generated/SettingChangedEventData";
export type { DeviceEventData } from "./generated/DeviceEventData";
export type { FilesOpenedEventData } from "./generated/FilesOpenedEventData";
export type { FilesSentEventData } from "./generated/FilesSentEventData";
export type { HostMember } from "./generated/HostMember";
export type { ScreenEventData } from "./generated/ScreenEventData";

/** The name of every event an add-on can subscribe to. */
export type AddonEventName = keyof AddonEventMap;

/** A function a script gives; it may be async, and one that fails is logged and does not stop the add-on. */
export type Callback<Args extends unknown[]> = (...args: Args) => void | Promise<void>;

// ---------------------------------------------------------------- the errors a call can throw

/** Registering a menu entry or a button was refused. */
export interface MenuRegisterError extends Error {
  name: "MenuRegisterError";
  code: MenuRegisterCode;
}

/** A program was not started. */
export interface ProcessError extends Error {
  name: "ProcessError";
  code: ProcessErrorCode;
}

/** A screen was not fed to a program. */
export interface ScreenError extends Error {
  name: "ScreenError";
  code: ScreenErrorCode;
}

/** The add-on was not given the permission the call needs. */
export interface PermissionDenied extends Error {
  name: "PermissionDenied";
  permission: AddonPermission;
}

/** A command on a device failed (`code` is the program's code for why, such as `command_failed`). */
export interface DeviceError extends Error {
  name: "DeviceError";
  code: string;
}

/** The folder of the add-on is not known: it is not installed any more, or its path is not valid text. */
export interface AddonFolderError extends Error {
  name: "AddonFolderError";
}

/** `host.on` was given a name that is not an event. */
export interface UnknownEvent extends Error {
  name: "UnknownEvent";
  event: string;
}

/** Too many timers wait at once. */
export interface TimerLimit extends Error {
  name: "TimerLimit";
}

// ---------------------------------------------------------------- what a call takes and gives

/** What every place that takes an entry (`deviceMenu`, `appMenu`, `toolbar`) is given. */
export interface EntryOf<Args extends unknown[]> {
  /** 1 to 64 letters, digits, `_` and `-`. Each place has names of its own. */
  id: string;
  /** What the user reads: 1 to 64 characters without line breaks. */
  label: string;
  onClick: Callback<Args>;
}

/** A program started with `host.process.spawn`. */
export interface ProcessHandle {
  /** Ends the program; whether it was running. */
  kill(): boolean;
  /** Closes its standard input (a program that reads to the end then ends); whether it was open. */
  closeInput(): boolean;
}

export interface ProcessOptions {
  /** The program, as the system finds it. No shell reads it. */
  command: string;
  /** Its arguments, each as it is. */
  args?: string[];
  /** A line the program wrote to its standard output (without the line end; at most 64 KiB of it). */
  onLine?: Callback<[line: string]>;
  /** The program ended: its exit code, or `null` when it was ended by this program or by a signal. */
  onExit?: Callback<[code: number | null]>;
}

export interface Host {
  /** Writes a line to the debug console of the program, marked with the id of the add-on. */
  log(...values: unknown[]): void;

  /** The menu on the card of a device: `onClick(device)`. */
  deviceMenu: { register(entry: EntryOf<[device: string]>): void };
  /** The menu of an app: `onClick(device, app)`, `app` being the package. */
  appMenu: { register(entry: EntryOf<[device: string, app: string]>): void };
  /** The toolbar of a screen: `app` is the package for the window of an app, `null` for the screen of the device. */
  toolbar: { register(entry: EntryOf<[device: string, app: string | null]>): void };

  /** Programs of this computer. Needs the permission `run_process`. */
  process: { spawn(options: ProcessOptions): ProcessHandle };
  /** The screen of a device. Needs the permission `screen_capture`. */
  screen: {
    /**
     * Feeds the live video (H.264, Annex B, as the device encodes it) to the input of a program from `spawn`.
     * `app` is the package for the window of an app (as a toolbar button is given it with the device); none or `null`
     * for the screen of the device itself.
     */
    pipeTo(process: ProcessHandle, device: string, app?: string | null): void;
  };

  /**
   * What the user set for this add-on (the settings it declared in `addon.json`). `get` gives the value now: a
   * boolean, a whole number or a text; `undefined` for a setting that was not declared. The event
   * `settings.changed` (`host.on`) tells of a change. Needs no permission.
   */
  settings: { get(key: string): SettingValue | undefined };

  /** This add-on itself. */
  addon: {
    /**
     * The path of the folder the add-on is installed from (where `addon.json` and `main.js` are), as the system
     * writes it. A program that comes with the add-on can be found there and started with `process.spawn`. Needs no
     * permission. Throws `AddonFolderError` when the add-on is not installed any more.
     */
    folder(): string;
  };

  /** Subscribes to an event; returns the function that cancels the subscription. Needs no permission. */
  on<Name extends AddonEventName>(name: Name, callback: Callback<[data: AddonEventMap[Name]]>): () => void;

  /** Runs a shell command on a device and gives its output (at most 1 MiB). Needs the permission `device_shell`. */
  deviceShell(device: string, command: string): Promise<string>;
}

declare global {
  const host: Readonly<Host>;

  const console: {
    log(...values: unknown[]): void;
    info(...values: unknown[]): void;
    debug(...values: unknown[]): void;
    warn(...values: unknown[]): void;
    error(...values: unknown[]): void;
  };

  function setTimeout(callback: () => void | Promise<void>, delay?: number): number;
  function setInterval(callback: () => void | Promise<void>, delay?: number): number;
  function clearTimeout(timer: number | undefined): void;
  function clearInterval(timer: number | undefined): void;
}
