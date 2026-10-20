// An add-on written against the types, to see that they take what the interface takes and refuse what it does not.
// It is only compiled (`npm run check:addon-types`); it is never run.

import type { MenuRegisterError, ProcessHandle } from "../index";

host.deviceMenu.register({
  id: "say-hello",
  label: "Say hello",
  onClick: async (device) => {
    const output: string = await host.deviceShell(device, "echo hello");
    host.log(device, output);
  },
});

host.appMenu.register({ id: "log-app", label: "Log", onClick: (device, app) => host.log(device, app.toUpperCase()) });
host.toolbar.register({ id: "log-screen", label: "Log", onClick: (device, app) => host.log(device, app ?? "device") });

const cancel = host.on("screen.started", (event) => host.log(event.device, event.app));
host.on("files.sent", (event) => (event.ok ? host.log(event.name) : host.log(event.error)));
cancel();

let program: ProcessHandle | null = null;
try {
  program = host.process.spawn({
    command: "node",
    args: ["-e", "0"],
    onLine: (line) => host.log(line.length),
    onExit: (code) => host.log(code === null ? "ended" : code + 1),
  });
  host.screen.pipeTo(program, "device");
  host.screen.pipeTo(program, "device", "com.example.app");
  host.screen.pipeTo(program, "device", null);
} catch (error) {
  const failure = error as MenuRegisterError;
  host.log(failure.name, failure.code);
}
program?.closeInput();
program?.kill();

setTimeout(() => host.log("later"), 1000);

// What the interface does not take is refused by the compiler.
// @ts-expect-error an event that does not exist
host.on("device.exploded", () => {});
// @ts-expect-error the callback is given the data of that event, which has no `file`
host.on("files.sent", (event) => event.file);
// @ts-expect-error a toolbar button is given the device and the app, not a number
host.toolbar.register({ id: "x", label: "X", onClick: (device: number) => host.log(device) });
// @ts-expect-error the command is text
host.process.spawn({ command: 1 });
// @ts-expect-error a handle from `spawn` is needed, not any object
host.screen.pipeTo({ id: 1 }, "device");
