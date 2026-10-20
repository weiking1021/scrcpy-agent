import { t } from "../shared/i18n";
import type { OpenSupport } from "../shared/types";

/** What to say about a device's answer to "can you open this file?", and whether it can be chosen. */
export function supportText(s: OpenSupport): { text: string; usable: boolean } {
  switch (s.kind) {
    case "apps":
      // A kind that is not known is still opened: the device decides, and says so.
      if (s.is_default) return { text: t("chooser.has_default"), usable: true };
      return {
        text: t(s.type_known ? "chooser.can_open" : "chooser.type_unknown", { count: s.count }),
        usable: true,
      };
    case "no_app":
      return { text: t("chooser.no_app"), usable: false };
    case "unreachable":
      return { text: t("chooser.unreachable"), usable: false };
  }
}
