// How the window of a device writes a time or a moment.

import { t } from "../../shared/i18n";

export function duration(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return t("device_window.duration_days", { days, hours });
  if (hours > 0) return t("device_window.duration_hours", { hours, minutes });
  return t("device_window.duration_minutes", { minutes });
}

/** A moment given in seconds since 1970; 0 means it is not known. */
export function moment(seconds: number): string {
  if (seconds === 0) return t("device_window.unknown_date");
  return new Intl.DateTimeFormat(document.documentElement.lang, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(seconds * 1000),
  );
}
