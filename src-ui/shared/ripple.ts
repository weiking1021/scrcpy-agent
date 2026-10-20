// The answer to a press: a ripple spreads from where the button was pressed (from
// its middle when it was pressed with the keyboard), so a press never looks like
// no press, whatever the button then does or however long that takes.

/** How long a ripple takes (the same as `.ripple` in styles.css). */
const RIPPLE_MS = 550;

/** Buttons that are only text in a line have nothing to ripple in. */
const SKIPPED = "button.link";

function ripple(button: HTMLElement, x: number, y: number): void {
  const box = button.getBoundingClientRect();
  // Big enough to cover the button from the point it started at.
  const radius = Math.hypot(Math.max(x - box.left, box.right - x), Math.max(y - box.top, box.bottom - y));
  const wave = document.createElement("span");
  wave.className = "ripple";
  wave.style.width = wave.style.height = `${radius * 2}px`;
  wave.style.left = `${x - box.left - radius}px`;
  wave.style.top = `${y - box.top - radius}px`;
  wave.addEventListener("animationend", () => wave.remove());
  button.appendChild(wave);
  // An animation that does not run (the button is hidden meanwhile) never ends: the ripple goes anyway.
  window.setTimeout(() => wave.remove(), RIPPLE_MS + 200);
}

function target(ev: Event): HTMLButtonElement | null {
  const button = (ev.target as HTMLElement | null)?.closest<HTMLButtonElement>("button");
  return button && !button.disabled && !button.matches(SKIPPED) ? button : null;
}

/** Makes every button of the page (also those made later) answer a press. */
export function initRipple(): void {
  document.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    const button = target(ev);
    if (button) ripple(button, ev.clientX, ev.clientY);
  });
  // A press with the keyboard is a click that has no position.
  document.addEventListener("click", (ev) => {
    if (ev.detail !== 0) return;
    const button = target(ev);
    if (!button) return;
    const box = button.getBoundingClientRect();
    ripple(button, box.left + box.width / 2, box.top + box.height / 2);
  });
}
