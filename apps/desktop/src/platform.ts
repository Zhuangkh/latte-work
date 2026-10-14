import { invoke } from "@tauri-apps/api/core";
import { native } from "./api";

let cached: Promise<string> | undefined;

/** Native OS name used for desktop chrome; resolves to "unknown" outside Tauri. */
export function desktopPlatform(): Promise<string> {
  if (!native) return Promise.resolve("unknown");
  cached ??= invoke<string>("desktop_platform").catch(() => "unknown");
  return cached;
}
