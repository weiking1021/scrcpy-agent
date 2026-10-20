The `.tgz` here is the types package `scrcpy-agent-addon-api` as it was when this example was written (1.0.0). Newer
ones are attached to each release of the program. `package.json` of the example installs this one from this folder, so
the example works as it is: `npm install`, then your editor knows `host`, `console` and the timers (`jsconfig.json`).

In a project of your own, put the `.tgz` of the release you use where you keep such files and run
`npm install --save-dev ./path/to/scrcpy-agent-addon-api-<version>.tgz`.
