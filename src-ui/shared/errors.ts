import { t } from "./i18n";
import { isCoreError } from "./types";
import type { AddonStatus, InstallErrorDetail } from "./types";

/**
 * The key of the text for a reason of the core that is a plain name (`not_a_folder`) or carries
 * another (`{ manifest: "bad_id" }`): `<prefix>.not_a_folder`, `<prefix>.manifest.bad_id`.
 */
export function addonReasonKey(prefix: string, reason: InstallErrorDetail | AddonStatus): string {
  if (typeof reason === "string") return `${prefix}.${reason}`;
  if ("manifest" in reason) return `${prefix}.manifest.${reason.manifest}`;
  return `${prefix}.unsupported.${reason.unsupported}`;
}

/** Message for a failed command: the translated text for its code, plus raw detail if any. */
export function describeError(e: unknown): { text: string; detail: string } {
  if (isCoreError(e)) {
    // The one error whose detail is a fact (the exit code), not text from adb.
    if (e.code === "command_failed") {
      return { text: t("error.command_failed", { code: e.detail }), detail: "" };
    }
    // The reason of an add-on's refusal is a value of the core, told in the language of the window.
    if (e.code === "addon") {
      return { text: t("error.addon"), detail: t(addonReasonKey("addon.error", e.detail)) };
    }
    // The reason a value was refused is a value of the core too.
    if (e.code === "invalid_setting") {
      return { text: t("error.invalid_setting"), detail: t(`addon.setting_problem.${e.detail}`) };
    }
    if (e.code === "scrcpy_release") {
      const reason = e.detail;
      const key = typeof reason === "string" ? `scrcpy.error.${reason}` : `scrcpy.error.offer.${reason.offer}`;
      return { text: t("error.scrcpy_release"), detail: t(key) };
    }
    return { text: t(`error.${e.code}`), detail: "detail" in e ? e.detail : "" };
  }
  // Not from the core (for example the IPC layer itself failed).
  return { text: t("error.other"), detail: String(e) };
}
