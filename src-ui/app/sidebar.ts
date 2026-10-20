// Folding and opening the sidebar. Folded, the sidebar is as wide as what is left in it (see
// `.sidebar` in styles.css), which only the browser knows once a theme has put its padding and frames
// on the entries; so the end width is asked of the browser, and only the width is moved to it.

const DURATION_MS = 150;

/** Stops the movement that is going on, if there is one (it leaves the sidebar as it is). */
let stop: (() => void) | null = null;

/**
 * Folds the sidebar or opens it. With `animate` the width moves to its new value: the labels stay on
 * the way (cut off at the edge as it narrows) and go, when folding, at its end.
 */
export function setFolded(sidebar: HTMLElement, folded: boolean, animate = true): void {
  // Where it is now, also when it is half way (the movement that is going on is dropped, not finished).
  const from = sidebar.getBoundingClientRect().width;
  stop?.();
  // The state it ends in, laid out by the browser.
  sidebar.classList.toggle("collapsed", folded);
  const to = sidebar.getBoundingClientRect().width;
  if (!animate || from === to || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

  sidebar.classList.add("moving");
  sidebar.style.width = `${from}px`;
  // Lets the browser take the width it starts from before the width it goes to is set.
  void sidebar.offsetWidth;
  sidebar.style.transition = `width ${DURATION_MS}ms ease`;
  sidebar.style.width = `${to}px`;

  const finish = () => {
    sidebar.removeEventListener("transitionend", onEnd);
    window.clearTimeout(timer);
    sidebar.style.transition = "";
    sidebar.style.width = "";
    sidebar.classList.remove("moving");
    stop = null;
  };
  const onEnd = (event: TransitionEvent) => {
    if (event.target === sidebar && event.propertyName === "width") finish();
  };
  sidebar.addEventListener("transitionend", onEnd);
  // In case the end of the movement is not announced (the window is hidden).
  const timer = window.setTimeout(finish, DURATION_MS + 100);
  stop = finish;
}
