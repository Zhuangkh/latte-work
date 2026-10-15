import { useSyncExternalStore } from "react";

// Compare calendar dates, rather than elapsed 24-hour periods (midnight/DST).
function localDay(date: Date) {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
}

export function formatMessageTimestamp(at: number, now = Date.now()) {
  const date = new Date(at);
  const today = new Date(now);
  if (!Number.isFinite(date.getTime()) || !Number.isFinite(today.getTime()))
    return null;
  const days = (localDay(today) - localDay(date)) / 86_400_000;
  const relative = ["今天", "昨天", "前天"][days];
  const day =
    relative ??
    `${date.getFullYear() === today.getFullYear() ? "" : `${date.getFullYear()}年`}${date.getMonth() + 1}月${date.getDate()}日`;
  const time = new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).format(date);
  return `${day} ${time}`;
}

// One shared calendar clock for all message rows, including idle history tabs.
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setTimeout> | undefined;
function refreshDate() {
  clearTimeout(timer);
  for (const listener of listeners) listener();
  if (!listeners.size) return;
  const now = new Date();
  const midnight = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() + 1,
  );
  timer = setTimeout(refreshDate, midnight.getTime() - now.getTime() + 1);
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    refreshDate();
    window.addEventListener("focus", refreshDate);
    document.addEventListener("visibilitychange", refreshDate);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      clearTimeout(timer);
      window.removeEventListener("focus", refreshDate);
      document.removeEventListener("visibilitychange", refreshDate);
    }
  };
}
const currentDay = () => localDay(new Date());

export function MessageTimestamp({
  at,
  label,
}: {
  at?: number;
  label: "发送时间" | "回复时间";
}) {
  useSyncExternalStore(subscribe, currentDay, currentDay);
  if (at === undefined) return null;
  const date = new Date(at);
  const text = formatMessageTimestamp(at);
  if (text === null) return null;
  return (
    <time
      dateTime={date.toISOString()}
      title={date.toLocaleString("zh-CN", { hour12: false })}
      aria-label={`${label}：${text}`}
    >
      {text}
    </time>
  );
}
