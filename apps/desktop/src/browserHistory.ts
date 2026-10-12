import { useEffect, useState } from "react";
import { webLink } from "./browserLinks";
export type BrowserVisit = {
  id: string;
  url: string;
  title: string;
  at: number;
};
const key = "latte-work.browser-history.v1";
const changed = "latte-work-browser-history";
const limit = 500;
export function readBrowserHistory(): BrowserVisit[] {
  const raw = localStorage.getItem(key);
  if (!raw) return [];
  if (raw.length > 5 * 1024 * 1024) throw new Error("浏览记录过大");
  const value: unknown = JSON.parse(raw);
  if (!Array.isArray(value)) throw new Error("浏览记录无效");
  return value
    .slice(-limit)
    .filter(
      (item): item is BrowserVisit =>
        !!item &&
        typeof item.id === "string" &&
        typeof item.url === "string" &&
        !!webLink(item.url) &&
        typeof item.title === "string" &&
        item.title.length <= 256 &&
        typeof item.at === "number" &&
        Number.isFinite(item.at),
    );
}
function save(items: BrowserVisit[]) {
  localStorage.setItem(key, JSON.stringify(items.slice(-limit)));
  window.dispatchEvent(new Event(changed));
}
export function recordBrowserVisit(url: string, title?: string) {
  const address = webLink(url);
  if (!address) return;
  const items = readBrowserHistory();
  const latest = items.at(-1);
  const label = (
    title ||
    (latest?.url === address && latest.title) ||
    new URL(address).hostname
  ).slice(0, 256);
  if (latest?.url === address) latest.title = label;
  else
    items.push({
      id: crypto.randomUUID(),
      url: address,
      title: label,
      at: Date.now(),
    });
  save(items);
}
export function updateBrowserVisitTitle(url: string, title: string) {
  const address = webLink(url);
  if (!address) return;
  const items = readBrowserHistory();
  const item = [...items].reverse().find((item) => item.url === address);
  if (!item || item.title === title) return;
  item.title = title.slice(0, 256);
  save(items);
}
export function removeBrowserVisit(id: string) {
  save(readBrowserHistory().filter((item) => item.id !== id));
}
export function clearBrowserHistory() {
  save([]);
}
export function useBrowserHistory(report: (error: unknown) => void) {
  const [items, setItems] = useState<BrowserVisit[]>([]);
  useEffect(() => {
    const reload = () => {
      try {
        setItems(readBrowserHistory());
      } catch (error) {
        report(error);
      }
    };
    reload();
    const storage = (event: StorageEvent) => {
      if (event.key === key) reload();
    };
    window.addEventListener(changed, reload);
    window.addEventListener("storage", storage);
    return () => {
      window.removeEventListener(changed, reload);
      window.removeEventListener("storage", storage);
    };
  }, [report]);
  return items;
}
