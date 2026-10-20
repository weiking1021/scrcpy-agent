// Numbers typed into the boxes of a page.

/** A whole number from `min` to `max`, else `null`. */
export function wholeNumberOrNull(text: string, min: number, max: number): number | null {
  const n = Number(text.trim());
  return text.trim() !== "" && Number.isInteger(n) && n >= min && n <= max ? n : null;
}

/** A whole number from `min` to `max`, else the one that was set before. */
export function wholeNumber(text: string, min: number, max: number, previous: number): number {
  return wholeNumberOrNull(text, min, max) ?? previous;
}
