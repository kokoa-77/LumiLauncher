// 时间和时长格式（首页、换世界、实例库共用）
import type { TFunction } from "i18next";
import type { WorldView } from "./bindings";
import { getUiLanguage } from "./i18n";

export function formatDuration(seconds: number, t: TFunction): string {
  const minutes = Math.floor(seconds / 60);
  if (minutes < 1) return t("home.world.lessThanMinute");
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return t("home.world.minutes", { m: minutes });
  return rest
    ? t("home.world.hoursMinutes", { h: hours, m: rest })
    : t("home.world.hours", { h: hours });
}

export function formatAgo(ms: number, t: TFunction): string {
  const minutes = (Date.now() - ms) / 60_000;
  if (minutes < 1) return t("home.world.justNow");
  if (minutes < 60) return t("home.world.minutesAgo", { n: Math.floor(minutes) });
  const hours = minutes / 60;
  if (hours < 24) return t("home.world.hoursAgo", { n: Math.floor(hours) });
  const days = hours / 24;
  if (days < 30) return t("home.world.daysAgo", { n: Math.floor(days) });
  return new Date(ms).toLocaleDateString(getUiLanguage());
}

/** 一个世界的几个短标签：第几天、模式、玩了多久、多久前 */
export function worldFacts(world: WorldView, t: TFunction): string[] {
  return [
    world.day != null ? t("home.world.day", { n: world.day }) : null,
    world.hardcore
      ? t("home.world.hardcore")
      : world.gameMode
        ? t(`home.world.mode.${world.gameMode}`)
        : null,
    world.playSeconds != null
      ? t("home.world.played", { time: formatDuration(world.playSeconds, t) })
      : null,
    world.lastPlayed ? formatAgo(world.lastPlayed, t) : null,
  ].filter((v): v is string => Boolean(v));
}

/** 文件大小：1 GB 以上写 GB、1 MB 以上写 MB（一位小数），以下写 KB */
export function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
  return bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** 短日期时间：中文「11月15日 14:03」，英文「Nov 15, 14:03」 */
export function formatDateTime(ms: number): string {
  const date = new Date(ms);
  const lang = getUiLanguage();
  const pad = (n: number) => String(n).padStart(2, "0");
  if (lang.toLowerCase().startsWith("zh")) {
    return `${date.getMonth() + 1}月${date.getDate()}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }
  return date.toLocaleString(lang, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

/** 数量简写：中文用万 / 亿，英文用 K / M / B（下载量、服务器在线人数共用） */
export function formatCount(value: number | null | undefined): string {
  const n = value ?? 0;
  if (getUiLanguage().toLowerCase().startsWith("zh")) {
    if (n >= 1e8) return `${(n / 1e8).toFixed(1)} 亿`;
    if (n >= 1e4) return `${(n / 1e4).toFixed(1)} 万`;
    return String(Math.round(n));
  }
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}
