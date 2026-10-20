import { invoke } from "@tauri-apps/api/core";
import { suppressNativeContextMenu } from "../shared/contextmenu";
import { describeError } from "../shared/errors";

suppressNativeContextMenu();
import { buttonIcon, buttonName } from "../shared/buttons";
import { loadUserPacks, setLanguage, t } from "../shared/i18n";
import { initTheme } from "../shared/theme";
import type { Snapshot, ToolbarButton } from "../shared/types";

const params = new URLSearchParams(location.search);
const device = params.get("device") ?? "";
const free = params.get("mode") === "free";

function labelOf(button: ToolbarButton): string {
  return buttonName(button.id, button.label);
}

/** Colours the button for a moment: it worked, or it did not. */
function flash(button: HTMLButtonElement, kind: "sent" | "failed"): void {
  button.classList.add(kind);
  setTimeout(() => button.classList.remove(kind), 600);
}

function render(buttons: ToolbarButton[]): void {
  const bar = document.getElementById("bar")!;
  bar.replaceChildren();
  if (free) {
    const grip = document.createElement("div");
    grip.className = "grip";
    // Tauri turns this attribute into "drag the window by here".
    grip.setAttribute("data-tauri-drag-region", "");
    bar.appendChild(grip);
  }
  for (const entry of buttons) {
    const button = document.createElement("button");
    // A button that cannot be pressed says why.
    const why = entry.unavailable === "needs_proxy" ? t("toolbar.needs_proxy") : t("toolbar.unavailable");
    const label = entry.enabled ? labelOf(entry) : `${labelOf(entry)} (${why})`;
    button.title = label;
    button.setAttribute("aria-label", label);
    button.disabled = !entry.enabled;
    button.innerHTML = buttonIcon(entry.id);
    button.addEventListener("click", async () => {
      try {
        await invoke("press_button", { device, button: entry.id });
        flash(button, "sent");
      } catch (e) {
        const { text, detail } = describeError(e);
        button.title = detail ? `${text}\n${detail}` : text;
        flash(button, "failed");
      }
    });
    bar.appendChild(button);
  }
}

async function main(): Promise<void> {
  void initTheme();
  await loadUserPacks();
  const state = await invoke<Snapshot>("get_snapshot");
  setLanguage(state.settings.language);
  render(await invoke<ToolbarButton[]>("toolbar_buttons", { device }));
}

main().catch((e) => console.error(e));
