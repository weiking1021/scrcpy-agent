// The notices the core sends (something happened that the user should know): each is a code, and each code has a
// message in the language packs.

import { t } from "../shared/i18n";
import { toast } from "../shared/toast";
import type { Notice, NoticeCode } from "../shared/types";

/** Every notice code has a message; the compiler enforces that none is missing. */
const NOTICE_TEXT: Record<NoticeCode, string> = {
  audio_unavailable: "notice.audio_unavailable",
  session_failed: "notice.session_failed",
  adb_start_failed: "notice.adb_start_failed",
  mdns_unavailable: "notice.mdns_unavailable",
  proxy_unavailable: "notice.proxy_unavailable",
  options_unavailable: "notice.options_unavailable",
  device_not_online: "notice.device_not_online",
};

export function showNotice(n: Notice): void {
  toast(n.level, t(NOTICE_TEXT[n.code]), n.detail);
}
