// The entry of the add-on. The program runs this file when the add-on starts (when the program starts, and
// when the add-on is installed or its permissions are decided again). Everything the script can do is in
// `host`, `console` and the timers (`setTimeout`, `setInterval`); there are no files, no network, no `require`.
// The lines it writes are in the debug console of the program, marked with the id of the add-on.

host.log("example-addon started");

// An entry in the menu of every device: a button on the card of the device in the device list (it works while the
// device is ready). The function runs when the user chooses the entry; `device` is the
// id of the device. Calls of one add-on run one at a time, and one that takes longer than a few seconds is stopped.
host.deviceMenu.register({
  id: "say-hello",
  label: "Say hello (example add-on)",
  onClick: async (device) => {
    // The settings are read when the entry is chosen, so a change the user made a moment ago is already in effect.
    const greeting = String(host.settings.get("greeting"));
    const line = host.settings.get("style") === "upper" ? greeting.toUpperCase() : greeting;
    for (let i = 0; i < Number(host.settings.get("repeat")); i++) {
      host.log(line + " from " + device);
    }

    // Running a command on the device needs the permission "device_shell" (see addon.json). Without it the call
    // fails with an error named PermissionDenied, which can be caught like any other.
    try {
      const output = await host.deviceShell(device, "echo hello from the device");
      host.log("the device said: " + output.trim());
    } catch (error) {
      host.log(error.name + (error.permission ? " " + error.permission : error.code ? " " + error.code : ""));
    }
  },
});

// An entry in the menu of an app (the apps of the device): `onClick(device, app)`, `app` being the package.
host.appMenu.register({
  id: "log-app",
  label: "Log this app (example add-on)",
  onClick: (device, app) => host.log("app menu: " + app + " on " + device),
});

// A button on the toolbar of a screen. The label is shown as its tip. `app` is the package when the screen is the
// window of an app, and null for the screen of the device itself.
host.toolbar.register({
  id: "log-screen",
  label: "Log this screen (example add-on)",
  onClick: (device, app) => host.log("toolbar: " + (app ?? "the screen of the device") + " on " + device),
});

// A program of this computer that keeps running, fed with the live video of a screen. It needs the permissions
// "run_process" and "screen_capture". The program here is `analyzer.js` of this folder, run by Node (any program that
// reads its input will do): it counts the bytes and the key frames of the H.264 stream and writes one line a second,
// which the add-on logs. `host.addon.folder()` is where this add-on is installed from, so a program that comes with
// it is found there. The toolbar button starts it for the screen it was pressed on, and ends it when pressed again.
// (The path is joined with `/`, which Windows takes as well.)
const ANALYZER = host.addon.folder() + "/analyzer.js";

let analyzer = null;

host.toolbar.register({
  id: "analyze-screen",
  label: "Analyze this screen (example add-on)",
  // `app` is the package when the button is on the toolbar of the window of an app, null on the screen of the device:
  // the same two arguments go to `pipeTo`, so it follows the screen the button belongs to.
  onClick: (device, app) => {
    if (analyzer) {
      analyzer.kill();
      analyzer = null;
      return;
    }
    try {
      // The command and each argument are taken as they are (no shell). `node` is found as the system finds it.
      const program = host.process.spawn({
        command: "node",
        args: [ANALYZER],
        onLine: (line) => host.log("analyzer: " + line),
        onExit: (code) => {
          host.log("analyzer ended, code " + code);
          if (analyzer === program) analyzer = null;
        },
      });
      analyzer = program;
      host.screen.pipeTo(program, device, app);
      host.log("analyzer started on " + (app ?? "the screen of") + " " + device);
    } catch (error) {
      host.log("could not start the analyzer: " + error.name + " " + (error.code ?? error.permission ?? error.message));
      if (analyzer) {
        analyzer.kill();
        analyzer = null;
      }
    }
  },
});

// A timer, as in a browser. This one runs once, a moment after the start.
setTimeout(() => console.log("a timer of the example add-on ran"), 1000);

// Events: what happens in the program is told to the add-ons that subscribe to it. Each subscription is a name and a
// function; the function is given an object with the members of that event. Events need no permission. When the
// add-on starts it is also told of what is the case already: `device.online` for each usable device and
// `screen.started` for each screen that is open. Every event is written to the debug console here.
//
//   device.online   { device, name }              a device became usable
//   device.offline  { device, name }              a usable device is not any more
//   device.added    { device, name }              this computer knows the device for the first time (not the
//                                                 pairing of adb: a device by USB is added too)
//   screen.started  { device, app }               a screen opened (`app` is the package for the window of an app, else null)
//   screen.ended    { device, app }               a screen closed
//   files.sent      { device, name, ok, error }   a file was sent to a device (`error` is the code of why not, else null)
//   files.opened    { device, file, ok, error }   a file was opened on a device
//   settings.changed { key, value }               the user changed a setting of this add-on (see below)
for (const name of /** @type {const} */ ([
  "device.online",
  "device.offline",
  "device.added",
  "screen.started",
  "screen.ended",
  "files.sent",
  "files.opened",
  "settings.changed",
])) {
  // `verbose` is a setting: the events are written only while the user has it on. A change of a setting is always told.
  host.on(name, (event) => {
    if (name === "settings.changed" || host.settings.get("verbose") === true) host.log(name + " " + JSON.stringify(event));
  });
}

// Settings: `addon.json` declares them (`greeting`, `repeat`, `style` and `verbose`); the program draws the form (the
// "Settings" button of this add-on in the settings window), checks what the user types and keeps it. `host.settings.get`
// gives the value now, always one the declaration allows; `undefined` for a key that was not declared. The entry
// "Say hello" reads three of them each time it runs; `verbose` is read by the subscriptions above.
host.log("settings at the start: " + ["greeting", "repeat", "style", "verbose"].map((k) => k + "=" + host.settings.get(k)).join(", "));

// `host.on` gives back a function that cancels the subscription. This one listens to the first device that becomes
// usable and then stops.
const cancel = host.on("device.online", (event) => {
  host.log("the first usable device since the start: " + event.name);
  cancel();
});

// Things to try with this folder:
//  - install it and leave "device_shell" unticked: choose the entry on a device card and read the console;
//  - the same, with it ticked;
//  - open the screen of a device and press "Analyze this screen" on its toolbar: the console shows a line a second
//    with the bytes and key frames received; press it again to end the program;
//  - remove "screen_capture" from addon.json, or add a permission to it, and open the settings;
//  - throw an error on the first line and install it again: the settings say why it did not start;
//  - delete this file and see the status of the add-on.
