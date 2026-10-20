// Small helpers every window uses to build its page.

/** The element with this id (the page is written to have it: a missing one is a mistake of the page). */
export const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

/** Text made safe to be put in HTML, in a text or in a quoted attribute. */
export const esc = (s: string): string =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
