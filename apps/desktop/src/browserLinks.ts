import { useSyncExternalStore } from "react";
export type LinkTarget = "internal" | "system";
const key = "latte-work.link-target.v1";
const listeners = new Set<() => void>();
export function readLinkTarget(): LinkTarget {
  try {
    return localStorage.getItem(key) === "system" ? "system" : "internal";
  } catch {
    return "internal";
  }
}
export function saveLinkTarget(target: LinkTarget) {
  localStorage.setItem(key, target);
  listeners.forEach((listener) => listener());
}
export function useLinkTarget() {
  return useSyncExternalStore((listener) => {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, readLinkTarget);
}
export function webLink(value: string): string | null {
  if (value.length > 8192) return null;
  try {
    const url = new URL(value);
    return ["https:", "http:"].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      !["tauri.localhost", "ipc.localhost"].includes(url.hostname)
      ? url.href
      : null;
  } catch {
    return null;
  }
}
// Address-bar input may omit the scheme; links elsewhere stay explicit.
export function browserAddress(value: string): string | null {
  const address = value.trim();
  if (!address) return null;
  if (/^https?:/i.test(address)) return webLink(address);
  const url = webLink(`https://${address}`);
  if (!url) return null;
  const hostname = new URL(url).hostname;
  return hostname.includes(".") ||
    hostname === "localhost" ||
    hostname.startsWith("[")
    ? url
    : null;
}
