/** The browser's own right-click menu (back, reload, print, inspect…) has no place in an app. */

/** Where text is typed, the menu stays: copy and paste are needed there. */
function isTextField(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
}

/** Hides the browser's menu everywhere but in text fields; development builds keep it for "Inspect". */
export function suppressNativeContextMenu(): void {
  if (import.meta.env.DEV) return;
  document.addEventListener("contextmenu", (ev) => {
    if (!isTextField(ev.target)) ev.preventDefault();
  });
}

export interface PopupEntry {
  label: string;
  enabled: boolean;
  /** A line is drawn above this entry. */
  separator?: boolean;
  run: () => void;
}

let close: (() => void) | null = null;

/** Closes the menu that is open, if any. */
export function closePopupMenu(): void {
  close?.();
}

/** Shows a menu at the point (kept inside the window); it closes on a choice, Escape, a click outside, scrolling or blur. */
export function showPopupMenu(x: number, y: number, entries: PopupEntry[], returnFocus?: HTMLElement): void {
  closePopupMenu();
  const menu = document.createElement("div");
  menu.className = "popup-menu";
  menu.setAttribute("role", "menu");
  const buttons: HTMLButtonElement[] = [];
  for (const entry of entries) {
    if (entry.separator) {
      const line = document.createElement("div");
      line.className = "popup-separator";
      line.setAttribute("role", "separator");
      menu.appendChild(line);
    }
    const b = document.createElement("button");
    b.type = "button";
    b.className = "popup-item";
    b.setAttribute("role", "menuitem");
    b.textContent = entry.label;
    b.disabled = !entry.enabled;
    b.addEventListener("click", () => {
      closePopupMenu();
      entry.run();
    });
    menu.appendChild(b);
    buttons.push(b);
  }
  document.body.appendChild(menu);
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - box.width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - box.height - 4))}px`;

  const enabled = buttons.filter((b) => !b.disabled);
  enabled[0]?.focus();
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closePopupMenu();
      returnFocus?.focus();
    } else if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      if (enabled.length === 0) return;
      const at = enabled.indexOf(document.activeElement as HTMLButtonElement);
      const step = ev.key === "ArrowDown" ? 1 : -1;
      enabled[(at + step + enabled.length) % enabled.length].focus();
    }
  };
  const onOutside = (ev: Event) => {
    if (!menu.contains(ev.target as Node)) closePopupMenu();
  };
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("scroll", closePopupMenu, true);
  window.addEventListener("blur", closePopupMenu);
  window.addEventListener("resize", closePopupMenu);
  close = () => {
    document.removeEventListener("keydown", onKey, true);
    document.removeEventListener("pointerdown", onOutside, true);
    document.removeEventListener("scroll", closePopupMenu, true);
    window.removeEventListener("blur", closePopupMenu);
    window.removeEventListener("resize", closePopupMenu);
    menu.remove();
    close = null;
  };
}
