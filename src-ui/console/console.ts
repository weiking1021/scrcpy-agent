// The debug console window: a page of its own, so it can stay open next to the
// main window while the user works. The core sends the log while this window
// is open and stops when it is closed.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { $ } from "../shared/dom";
import { describeError } from "../shared/errors";
import { loadUserPacks, setAppName, setLanguage, t, translateDom } from "../shared/i18n";
import { initRipple } from "../shared/ripple";
import { initTheme } from "../shared/theme";
import { LOG_LEVELS, isLogLevel } from "../shared/types";
import type { CoreEvent, LogLine, LogLevel, Snapshot } from "../shared/types";

/** How many lines the console keeps. */
const MAX_LINES = 1000;

let lines: LogLine[] = [];
let paused = false;
let shownLevel: LogLevel = "debug";

function two(n: number, width = 2): string {
  return String(n).padStart(width, "0");
}

function format(line: LogLine): string {
  const d = new Date(line.at_ms);
  const time = `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}.${two(d.getMilliseconds(), 3)}`;
  return `${time} ${line.level.padEnd(5)} ${line.code} ${line.message}`;
}

/** Lines at the chosen level or more important ones. */
function visible(): LogLine[] {
  const limit = LOG_LEVELS.indexOf(shownLevel);
  return lines.filter((l) => LOG_LEVELS.indexOf(l.level) <= limit);
}

function render(): void {
  const el = $("console-lines");
  const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  el.textContent = visible().map(format).join("\n") || t("console.empty");
  if (atBottom) el.scrollTop = el.scrollHeight;
}

function add(line: LogLine): void {
  lines.push(line);
  if (lines.length > MAX_LINES) lines.splice(0, lines.length - MAX_LINES);
  // While paused the lines are still collected; they show when it is resumed.
  if (!paused) render();
}

function setPaused(value: boolean): void {
  paused = value;
  $("console-pause").textContent = t(paused ? "console.resume" : "console.pause");
  if (!paused) render();
}

function showError(e: unknown): void {
  const { text, detail } = describeError(e);
  $("console-lines").textContent = detail ? `${text}\n${detail}` : text;
}

function wireControls(): void {
  const level = $<HTMLSelectElement>("console-level");
  $("console-pause").addEventListener("click", () => setPaused(!paused));
  $("console-clear").addEventListener("click", () => {
    lines = [];
    render();
  });
  $("console-copy").addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(visible().map(format).join("\n"));
      $("console-copy").textContent = t("console.copied");
      setTimeout(() => ($("console-copy").textContent = t("console.copy")), 1500);
    } catch (e) {
      showError(e);
    }
  });
  level.addEventListener("change", () => {
    if (isLogLevel(level.value)) {
      shownLevel = level.value;
      render();
    }
  });
}

async function main(): Promise<void> {
  initRipple();
  void initTheme();
  await loadUserPacks();
  const state = await invoke<Snapshot>("get_snapshot");
  setLanguage(state.settings.language);
  setAppName(state.settings.app_name);
  translateDom();
  await getCurrentWindow().setTitle(t("console.title"));
  wireControls();

  // Listen first, then ask for the log so far: the core hands over what it has
  // and sends only newer lines as events.
  await listen<CoreEvent>("core-event", (e) => {
    if (e.payload.kind === "log") add(e.payload.data);
  });
  const history = await invoke<LogLine[]>("open_debug_console");
  // Lines may have arrived as events before the answer did; they are newer
  // than the history (their numbers are larger), and none is shown twice.
  const lastSeq = history.length ? history[history.length - 1].seq : 0;
  lines = [...history, ...lines.filter((l) => l.seq > lastSeq)].slice(-MAX_LINES);
  render();
  const el = $("console-lines");
  el.scrollTop = el.scrollHeight;
}

main().catch(showError);
