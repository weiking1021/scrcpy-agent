// The rule for the name of the program, the same as `core/app_name.rs` (which is the one that decides:
// a name the core refuses is lost there). This copy only lets the settings window warn before saving.

export const MAX_APP_NAME_CHARS = 40;

/** What a file name cannot hold: the name is also the name of a submenu and of entries in file managers. */
const NOT_IN_A_FILE_NAME = new Set(["/", "\\", ":", "*", "?", '"', "<", ">", "|"]);

/** True for a control character (the C0 and C1 ranges, and DEL), as Rust's `char::is_control`. */
function isControl(char: string): boolean {
  const code = char.codePointAt(0) ?? 0;
  return code < 0x20 || (code >= 0x7f && code <= 0x9f);
}

/** The usable name in a typed text (trimmed), or null if the text cannot be a name. */
export function parseAppName(text: string): string | null {
  const name = text.trim();
  const chars = [...name];
  const usable =
    chars.length > 0 &&
    chars.length <= MAX_APP_NAME_CHARS &&
    !chars.some((c) => isControl(c) || NOT_IN_A_FILE_NAME.has(c));
  return usable ? name : null;
}
