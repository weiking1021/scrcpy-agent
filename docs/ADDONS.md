# Writing an add-on

**English** | [繁體中文](ADDONS.zh-TW.md)

This document is for add-on authors.

## The folder

An add-on is a folder with at least two files:

```
my-add-on/
  addon.json   the identity of the add-on and the permissions it asks for
  main.js      the code of the add-on
```

Other files are not restricted. The entry is always `main.js` and cannot be renamed. `main.js` is a single file of at
most 4 MiB (a larger file is not read, and the add-on does not start). TypeScript or packages must be bundled into it beforehand with a
toolchain (`tsc`, `esbuild`, …) targeting ES2022. The program does not compile TypeScript, and there is no `require`.

A minimal add-on:

```json
{ "id": "hello", "name": "Hello", "version": "1.0.0", "api": 1 }
```

```js
host.deviceMenu.register({
  id: "say-hello",
  label: "Say hello",
  onClick: (device) => host.log("hello from " + device),
});
```

## addon.json

```json
{
  "id": "my-addon",
  "name": "My add-on",
  "version": "1.0.0",
  "api": 1,
  "permissions": ["device_shell"],
  "platforms": ["windows", "linux"]
}
```

| Field | Required | Rule |
|---|---|---|
| `id` | yes | 1 to 64 letters, digits, `_` and `-`. It cannot change after the install. |
| `name` | yes | 1 to 60 characters, no control characters; spaces at both ends are removed. |
| `version` | yes | A string of 1 to 40 characters. It is only shown; the program does not compare versions. |
| `api` | yes | A whole number, 1 or more: the version of the add-on interface the add-on targets. The program supports `1`; a newer version does not run. |
| `permissions` | no | The permissions it asks for, no duplicates; none written means it asks for none. |
| `platforms` | no | The systems it supports (`windows`, `linux`, `android`), at least one, no duplicates; none written means all. |
| `settings` | no | What the user can adjust for this add-on (see the next section); none written means none. A declaration that is not valid makes the whole `addon.json` not valid. |

Other fields are ignored. An `addon.json` larger than 64 KiB is not read. If any field is not valid, the install is
refused and the reason names the field.

### Settings (`settings`)

With `settings` declared in `addon.json`, the program adds a "Settings" button to the row of the add-on in the settings
window, draws the form, checks the values the user types and keeps them. `main.js` reads a value with
`host.settings.get("id")` and is told of a change by the event `settings.changed`. A setting is an object: `id` and
`label` are required, `hint` (one line of help in the form) is optional, and `type` is one of these four:

| `type` | Required fields | Value |
|---|---|---|
| `boolean` | `default` (true or false) | a switch |
| `number` | `default`, `min`, `max` (whole numbers, `min` ≤ `default` ≤ `max`; a number with a fraction, such as `5.0`, is refused) | a whole number |
| `text` | `default` (text), `max_length` (1 to 1000) | one line of text (no line breaks or other control characters); `default` obeys the same rules and `max_length` |
| `choice` | `options` (a list of `{ "value", "label" }`, 1 to 50), `default` (one of the `value`s) | the `value` of one option |

```json
"settings": [
  { "id": "threshold", "type": "number", "label": "Threshold", "default": 5, "min": 0, "max": 100 },
  { "id": "verbose", "type": "boolean", "label": "Write more to the console", "default": false },
  { "id": "mode", "type": "choice", "label": "Mode", "default": "fast",
    "options": [{ "value": "fast", "label": "Fast" }, { "value": "slow", "label": "Slow" }] }
]
```

Rules:

- A setting `id`, and the `value` of an option, follow the rule of the `id` of an add-on (1 to 64 letters, digits, `_`
  and `-`). Setting ids are not repeated, and neither are the values within one `choice`.
- There are at most 50 settings. A `label` (also the `label` of an option) is 1 to 100 characters on one line. A `hint`,
  when present, is 1 to 300 characters on one line.
- The `default` in the declaration must be a value the setting allows.
- A kept value is checked against the current declaration each time it is read. If the declared range changes and a kept
  value no longer fits, that setting goes back to its default and the others are not touched.
- When the user types a value that is not allowed, the form states the reason and puts the previous value back.
  `main.js` only ever reads valid values.

## What main.js can use

`main.js` runs in the JavaScript engine built into the program (QuickJS). Each add-on has an environment and a thread of
its own and cannot see the others. The runtime holds at most 32 MiB of memory; a script that goes beyond is stopped. The
language itself is available (`Promise`, `async`/`await`, `JSON`, `Map`, …). There are no files, no network, and no
`require`, `process`, `fetch` or `Buffer`. Everything that reaches outside the engine is listed below.

### When it runs

- `main.js` runs once at the start: when the program starts, when the add-on is installed, and when the user decides its
  permissions again (it is stopped and started again then). After `main.js` returns, work started with promises or timers
  continues. A `main.js` that runs longer than 5 seconds is interrupted, the add-on does not start, and the settings
  window states why.
- Registered callbacks (a menu entry chosen, a timer) run one at a time, in the order they arrive. Each runs at most
  5 seconds (waiting included); a longer one is interrupted and logged, and the following callbacks run as usual.
- When the add-on is removed or started again, what it registered and its timers are cleared.

### `host`

| | Permission | What it does |
|---|---|---|
| `host.log(...values)` | no | Writes a line to the debug console, marked with this add-on. Values become text; a line has at most 2000 characters, and a longer one is cut and ends with `…`. |
| `host.deviceMenu.register({ id, label, onClick })` | no | Adds a button to the card of a device in the device list. `id` and `label` are text (`id` follows the rule of the `id` of an add-on, `label` is 1 to 64 characters), `onClick(device)` is a function, and `device` is the id of the device (an opaque text given by the program; it is the same text `deviceShell` and the events use). |
| `host.appMenu.register({ id, label, onClick })` | no | Adds an entry to the menu of an app. Same rules; `onClick(device, app)`, `app` being the package name. |
| `host.toolbar.register({ id, label, onClick })` | no | Adds a button to the toolbar of a screen (a generic icon; `label` is the tip). `onClick(device, app)`: `app` is the package name for the window of an app and `null` for the screen of the device itself. |
| `await host.deviceShell(device, command)` | `device_shell` | Runs a command in the shell of the device, as written, and gives its standard output (at most 1 MiB). An exit code other than 0 gives a `DeviceError` (`code` is `command_failed`). |
| `host.process.spawn({ command, args, onLine, onExit })` | `run_process` | Starts a long-running program on this computer and returns a handle `{ kill(), closeInput() }`. `command` and each of `args` are text, handed to the system as they are (no shell). `onLine(line)` gets each line of its standard output (at most 64 KiB a line; the rest of a longer one is dropped), and `onExit(code)` is called when it ends; the code is `null` when it was ended by the program or by a signal. At most 4 at a time for each add-on; they end when the add-on stops, is disabled or removed. A program that does not start throws `ProcessError`, whose `code` is `invalid_command`, `not_found`, `not_allowed`, `too_many` or `failed`. |
| `host.screen.pipeTo(handle, device, app?)` | `screen_capture` | Keeps writing the live video of a device's screen (H.264, Annex B, as the device encodes it; it does not pass through the JS of the add-on) to the standard input of that program. When the program reads slower than the screen, pictures are dropped until the next key frame. If the device has no open screen to follow, it throws `ScreenError` (`code` is `no_screen`); a handle that is not a program of this add-on gives `no_process`. |
| `host.settings.get(id)` | no | The value the user set for this add-on (a setting declared in `addon.json`): a boolean, a whole number or a text. An `id` that is not declared gives `undefined`; an `id` that is not text is a `TypeError`. A setting never set is its declared default. |
| `host.addon.folder()` | no | The path of the folder the add-on is installed from (where `addon.json` and `main.js` are), as the system writes it (`C:\...` on Windows). A program that comes with the add-on is there and can be started with `host.process.spawn`. A path that is not valid text, or an add-on no longer in the installed list, throws `AddonFolderError`. |

The three places to register have a namespace each (the same `id` can be registered once in each). A repeated
registration, or an `id` or `label` that breaks the rules, throws an error named `MenuRegisterError`, whose `code` is
`invalid_id`, `invalid_label` or `already_registered`. When the add-on is disabled or removed, the program takes back
everything it registered and every program it started.

### Events: `host.on(name, function)`

What happens in the program is told to the add-ons that subscribed. Events need no permission, and they carry no serial
number and no network address of a device. `host.on` returns a function that cancels the subscription (calling it twice
is fine). When an event has several subscriptions they run one after the other in the order of subscribing; if one
fails the others are not affected, and the error is written to the debug console with the name of the event. A name that
is not known gives an error named `UnknownEvent` (with an `event` field); a name or function of the wrong type is a
`TypeError`.

| Event | Data | When |
|---|---|---|
| `device.online` | `{ device, name }` | A device became usable |
| `device.offline` | `{ device, name }` | A usable device went offline, vanished, or is not usable any more (for example, not authorized) |
| `device.added` | `{ device, name }` | This computer knows the device for the first time (not the pairing of adb, which happens on the phone; a device by USB counts too) |
| `screen.started` | `{ device, app }` | A screen opened; `app` is the package for the window of an app, `null` for the screen of the whole device |
| `screen.ended` | `{ device, app }` | A screen closed |
| `files.sent` | `{ device, name, ok, error }` | A file finished sending (the progress is not an event); when it failed, `error` is the code (for example `not_online`) |
| `files.opened` | `{ device, file, ok, error }` | A file was opened on a device (or failed to) |
| `settings.changed` | `{ key, value }` | The user changed one setting of this add-on (told only after it was kept; only this add-on is told, it is not a broadcast) |

- `device` is the id of the device (the same as `deviceShell` takes); `name` is the name the program shows (a name the
  user gave comes first).
- Events about devices come from the device list before and after a poll, so they arrive one or two seconds after what
  happened.
- When an add-on starts it is told what is the case already: one `device.online` for each usable device and one
  `screen.started` for each open screen (not `device.added`). A reload does the same.
- A busy add-on keeps at most 100 events waiting; beyond that the oldest are dropped, and a line in the console says how
  many.

### `console`

`console.log`, `info`, `debug` (information), `warn` (warning), `error` (error) write to the debug console like
`host.log`, with the matching level.

### Timers

`setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`, used as in a browser. At most 100 timers at once; the
shortest `setInterval` is 10 milliseconds (a shorter one counts as 10); the longest delay is one day.

### Errors

A call that fails throws an ordinary `Error`. Its `name` is one of these, so a `try/catch` can decide by `name` and the
other fields:

| `name` | Other fields | When |
|---|---|---|
| `PermissionDenied` | `permission`: the name of the permission | A call needs a permission the user did not give. |
| `DeviceError` | `code`: the code of the core (for example `not_online`, `unknown_device`, `command_failed`) | An operation on a device failed. |
| `MenuRegisterError` | `code`: `invalid_id`, `invalid_label`, `already_registered` | A menu entry was not accepted. |
| `ProcessError` | `code`: `invalid_command`, `not_found`, `not_allowed`, `too_many`, `failed` | A program did not start. |
| `ScreenError` | `code`: `no_screen`, `no_process` | A screen was not fed to a program. |
| `UnknownEvent` | `event`: the name that was given | The name given to `host.on` is not an event. |
| `TimerLimit` | | Too many timers. |
| `AddonFolderError` | | `host.addon.folder()` does not know the folder of this add-on. |
| `TypeError` | | An argument has the wrong type (for example `onClick` is not a function). |

An error nobody catches is handled by where it happens: in a callback it is logged as an error line (with the error and
the stack) and only that callback is stopped; at the top level of `main.js` the add-on does not start; a rejected
promise nobody handles is logged as an error. An error in one add-on does not affect the program or another add-on.

## Permissions

| Name | Meaning |
|---|---|
| `device_shell` | Run shell commands on a device (`host.deviceShell`). |
| `run_process` | Start programs on this computer (`host.process.spawn`). A program can do anything the user can do on this computer, so the install shows a prominent warning. |
| `screen_capture` | Get the live video of a device's screen (`host.screen.pipeTo`). A screen can show messages, passwords, anything, so it is a permission of its own. |

- Registering menu entries and buttons needs no permission.
- At the install the user sees the permissions the add-on asks for and can untick each one. A call to an API whose
  permission was not given gets `PermissionDenied`, which `try/catch` can handle by doing less. The program records
  which calls without a permission an add-on tried, and the list in the settings window shows it.
- If `addon.json` later asks for more permissions, the list shows that the add-on asks for more than was allowed, and
  the add-on does not run until the user decides again. Asking for less changes nothing.

## TypeScript types

[../addon-api/](../addon-api/) holds the type definitions of `host`, `console`, the timers and the errors (the package
`scrcpy-agent-addon-api`; its major version matches the `api` of `addon.json`). It is not published to npm; it is
distributed as a `.tgz`, attached to each release (`npm run addon-types` builds it into `out/`, and needs no Docker).
Install it in the add-on's own project (the path is where the file is kept):

```
npm install --save-dev ./scrcpy-agent-addon-api-1.0.0.tgz
```

Then, in the config of that project (`tsconfig.json`, or `jsconfig.json` for a JavaScript project), set
`"lib": ["ES2022"]` without `"dom"` (a script runs with no browser and no Node, and those type sets would promise what
is not there) and `"types": ["scrcpy-agent-addon-api"]`. The global `host`, `console` and the timers then have types.
A bundler (for example esbuild) compiles TypeScript into the single `main.js` that goes in the add-on folder.

[example-addon/](example-addon/) shows this setup and can be copied as a starting point: its `package.json` installs a
1.0.0 `.tgz` from its own `vendor/` (`npm install` is enough) and its `jsconfig.json` names only the package. The example
is fixed at the types version it was written for, is not updated with the types, and is not part of any check. An
add-on needs no `package.json`; the program reads only `addon.json` and `main.js`.

The types of data (permissions, what each event carries, the `code` of each error) are generated from the types of the
program, so they match its behavior; the shape of the functions is checked against the list of functions the program
provides.

## The example

[example-addon/](example-addon/) is an add-on that can be installed as it is:

- a device button "Say hello (example add-on)" that logs a greeting according to the settings (the greeting, how many
  times, the style) and, with the permission `device_shell`, runs `echo` on the device;
- an app menu entry and a toolbar button;
- a toolbar button "Analyze this screen (example add-on)" that starts `analyzer.js`, which comes with the add-on (found
  with `host.addon.folder()`), and sends the screen to it; it runs with Node, so `node` must be available on this
  computer;
- a timer;
- four settings (`settings` in `addon.json`); the setting `verbose` decides whether all eight events are written to the
  console.

`main.js` lists a few things to try.

## Installing to test

Settings window → "Add-ons" → "Add" → "From a folder", and choose the folder of the add-on.

- The install is a link, not a copy: the program remembers the folder and reads its state again each time the settings
  window opens. After `main.js` changes, "Reload" on the add-on applies it: it is read from the folder and started again,
  the old registrations and timers are cleared, and the permissions the user gave stay (if `addon.json` now asks for
  more, the list shows that it asks for more than was allowed, and it does not run until the user decides again).
- "Disable" stops an add-on temporarily without removing it. The add-on stops and the menus and buttons it registered
  are taken back, but the install and the permissions stay; it is still disabled after the program restarts, and neither
  "Reload" nor deciding the permissions again makes it run while it is disabled. After "Enable", `main.js` runs again.
- Removing only forgets the link and stops the add-on; it does not touch the folder of the add-on, and it does not
  delete the data the add-on kept.
- Each add-on in the list has a state: ready, folder missing, `addon.json` missing or broken (the reason names the
  field), id changed, `main.js` missing, not supported on this system or version, more permissions asked. A ready add-on
  also shows whether it is running or not started (with the reason).
- The install, the removal, deciding permissions again, the logs of the add-on and the calls that were refused are all in
  the debug console ("Debug console" in the side bar).
