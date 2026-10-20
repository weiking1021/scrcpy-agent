// Runs once for each add-on, before its main.js. `native` holds the functions the program gives it; this
// builds the objects the script sees (`host`, `console` and the timers) out of them and returns the function
// the program calls to run a callback. `native` is closed over: the script cannot reach it.
(function (native) {
  "use strict";

  // What was registered: the callbacks, by the number the program knows them by.
  const menus = new Map();
  const timers = new Map();
  // The callbacks of the programs, by the number the program knows them by, and the number of each handle given out
  // (a handle that was not given out by `process.spawn` is not taken).
  const processes = new Map();
  const handles = new WeakMap();
  // The callbacks of each event, in the order they were subscribed.
  const subscriptions = new Map();
  let nextKey = 1;

  // An error with a name the script can switch on, and the facts of it as members.
  const fail = (name, members) => {
    const error = new Error(name);
    error.name = name;
    Object.assign(error, members);
    throw error;
  };

  const text = (value) => (typeof value === "string" ? value : String(value));

  // Levels of the log, as the program numbers them: 0 information, 1 warning, 2 error.
  const logger = (level) => (...values) => {
    native.log(level, values.map(text).join(" "));
  };

  // A place an add-on can put something in (`deviceMenu`, `appMenu`, `toolbar`): each has names of its own, and
  // `register` takes `{ id, label, onClick }`.
  const place = (what, register) =>
    Object.freeze({
      register(entry) {
        if (entry === null || typeof entry !== "object") throw new TypeError(`${what} must be an object`);
        if (typeof entry.id !== "string" || typeof entry.label !== "string") {
          throw new TypeError(`the id and the label of ${what} must be text`);
        }
        if (typeof entry.onClick !== "function") throw new TypeError("onClick must be a function");
        const key = nextKey++;
        const code = register(entry.id, entry.label, key);
        if (code) fail("MenuRegisterError", { code });
        menus.set(key, entry.onClick);
      },
    });

  const host = {
    log: logger(0),

    // The menu on the card of a device: `onClick(device)`.
    deviceMenu: place("a menu entry", (id, label, key) => native.registerMenu(id, label, key)),
    // The menu of an app: `onClick(device, app)`, `app` being the package.
    appMenu: place("a menu entry", (id, label, key) => native.registerAppMenu(id, label, key)),
    // The toolbar of a screen: `onClick(device, app)`, `app` being the package for the window of an app and
    // `null` for the screen of the device.
    toolbar: place("a toolbar button", (id, label, key) => native.registerToolbar(id, label, key)),

    // Programs of this computer (needs the permission `run_process`). The command and each argument are taken as
    // they are; no shell reads them. Returns a handle: `kill()`, `closeInput()`.
    process: Object.freeze({
      spawn(options) {
        if (options === null || typeof options !== "object") throw new TypeError("spawn needs an object");
        if (typeof options.command !== "string") throw new TypeError("the command must be text");
        const args = options.args === undefined ? [] : options.args;
        if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) {
          throw new TypeError("args must be a list of text");
        }
        for (const name of ["onLine", "onExit"]) {
          if (options[name] !== undefined && typeof options[name] !== "function") {
            throw new TypeError(name + " must be a function");
          }
        }
        const key = nextKey++;
        const answer = JSON.parse(native.spawnProcess(options.command, JSON.stringify(args), key));
        if (answer.denied) fail("PermissionDenied", { permission: answer.denied });
        if (!answer.ok) fail("ProcessError", { code: answer.code });
        processes.set(key, { onLine: options.onLine, onExit: options.onExit });
        const handle = Object.freeze({
          kill: () => native.killProcess(answer.id),
          closeInput: () => native.closeProcessInput(answer.id),
        });
        handles.set(handle, answer.id);
        return handle;
      },
    }),

    // The screen of a device (needs the permission `screen_capture`).
    screen: Object.freeze({
      // Feeds the live video (H.264, as the device encodes it) to the input of a program started by `process.spawn`.
      // `app` is the package for the window of an app (as a toolbar button is given it), none for the screen of the device.
      pipeTo(handle, device, app) {
        const id = handles.get(handle);
        if (id === undefined) throw new TypeError("pipeTo needs the handle of a program from host.process.spawn");
        if (typeof device !== "string") throw new TypeError("the device must be text");
        if (app !== undefined && app !== null && typeof app !== "string") throw new TypeError("the app must be text or null");
        const answer = JSON.parse(native.pipeScreen(device, id, app ?? null));
        if (answer.denied) fail("PermissionDenied", { permission: answer.denied });
        if (!answer.ok) fail("ScreenError", { code: answer.code });
      },
    }),

    // What the user set for this add-on (the settings it declared in addon.json). `get` gives the value now: a
    // boolean, a whole number or a text; `undefined` for a setting that was not declared. The event `settings.changed`
    // (`host.on`) tells of a change.
    settings: Object.freeze({
      get(key) {
        if (typeof key !== "string") throw new TypeError("the name of a setting must be text");
        const value = native.getSetting(key);
        return value === null || value === undefined ? undefined : JSON.parse(value);
      },
    }),

    // This add-on itself. `folder()` is the path of the folder it is installed from (where `addon.json` and `main.js`
    // are), as the system writes it: a program of its own that the add-on starts can be found there.
    addon: Object.freeze({
      folder() {
        const path = native.getFolder();
        if (path === null || path === undefined) fail("AddonFolderError", {});
        return path;
      },
    }),

    // Subscribes to an event; returns the function that cancels the subscription.
    on(name, callback) {
      if (typeof name !== "string") throw new TypeError("the name of an event must be text");
      if (typeof callback !== "function") throw new TypeError("a subscription needs a function");
      if (!native.isEvent(name)) fail("UnknownEvent", { event: name });
      const key = nextKey++;
      let callbacks = subscriptions.get(name);
      if (!callbacks) {
        callbacks = new Map();
        subscriptions.set(name, callbacks);
      }
      callbacks.set(key, callback);
      return () => {
        callbacks.delete(key);
      };
    },

    async deviceShell(device, command) {
      if (typeof device !== "string" || typeof command !== "string") {
        throw new TypeError("the device and the command must be text");
      }
      const answer = JSON.parse(await native.shell(device, command));
      if (answer.ok) return answer.output;
      if (answer.denied) fail("PermissionDenied", { permission: answer.denied });
      return fail("DeviceError", { code: answer.code });
    },
  };

  const console = {
    log: logger(0),
    info: logger(0),
    debug: logger(0),
    warn: logger(1),
    error: logger(2),
  };

  const addTimer = (callback, delay, repeat) => {
    if (typeof callback !== "function") throw new TypeError("a timer needs a function");
    const milliseconds = Number(delay);
    const key = nextKey++;
    const refused = native.startTimer(key, Number.isFinite(milliseconds) && milliseconds > 0 ? milliseconds : 0, repeat);
    if (refused) fail("TimerLimit", {});
    timers.set(key, { callback, repeat });
    return key;
  };

  const clearTimer = (key) => {
    if (timers.delete(key)) native.clearTimer(key);
  };

  const give = (name, value) => {
    Object.defineProperty(globalThis, name, { value, writable: false, configurable: false, enumerable: false });
  };
  give("host", Object.freeze(host));
  give("console", Object.freeze(console));
  give("setTimeout", (callback, delay) => addTimer(callback, delay, false));
  give("setInterval", (callback, delay) => addTimer(callback, delay, true));
  give("clearTimeout", clearTimer);
  give("clearInterval", clearTimer);

  const describe = (error) => {
    try {
      if (error instanceof Error) {
        const stack = typeof error.stack === "string" && error.stack !== "" ? "\n" + error.stack : "";
        return error.name + ": " + error.message + stack;
      }
      return text(error);
    } catch (_) {
      return "an error that cannot be shown";
    }
  };

  // Runs a callback and tells how it ended: null if it did, else what went wrong. It never rejects, so a
  // failure is reported once, by the program, and not also as an unhandled rejection.
  return async function dispatch(kind, key, argument) {
    if (kind === "event") {
      // An event runs every callback that is subscribed, one after the other; one that fails does not stop the
      // others, and what went wrong is told at the end.
      const { name, data } = JSON.parse(argument);
      const failures = [];
      for (const callback of [...(subscriptions.get(name)?.values() ?? [])]) {
        try {
          await callback(data);
        } catch (error) {
          failures.push(describe(error));
        }
      }
      return failures.length === 0 ? null : failures.join("\n");
    }
    try {
      let callback;
      if (kind === "processLine" || kind === "processExit") {
        const entry = processes.get(key);
        if (kind === "processExit") processes.delete(key);
        const callback = kind === "processLine" ? entry?.onLine : entry?.onExit;
        if (callback) await callback(...JSON.parse(argument));
        return null;
      }
      if (kind === "appMenu" || kind === "toolbar") {
        callback = menus.get(key);
        if (callback) await callback(...JSON.parse(argument));
        return null;
      }
      if (kind === "menu") {
        callback = menus.get(key);
      } else {
        const timer = timers.get(key);
        if (timer) {
          if (!timer.repeat) timers.delete(key);
          callback = timer.callback;
        }
      }
      if (callback) await callback(argument);
      return null;
    } catch (error) {
      return describe(error);
    }
  };
})
