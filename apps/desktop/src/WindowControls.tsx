import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";
import { desktopPlatform } from "./platform";

export function windowControlsVisible(platform: string | undefined) {
  return platform === "windows" || platform === "linux";
}

/**
 * Native window controls for undecorated windows. macOS keeps its traffic lights
 * and renders nothing here; browser tests inject the platform explicitly.
 */
export function WindowControls({
  platform: override,
}: { platform?: string } = {}) {
  const [detected, setDetected] = useState<string>();
  const [maximized, setMaximized] = useState(false);
  const platform = override ?? detected;
  const visible = windowControlsVisible(platform);
  useEffect(() => {
    if (override !== undefined) return;
    let alive = true;
    void desktopPlatform().then((value) => {
      if (alive) setDetected(value);
    });
    return () => {
      alive = false;
    };
  }, [override]);
  useEffect(() => {
    if (!visible) return;
    let alive = true;
    let unlisten: (() => void) | undefined;
    const refresh = () => {
      void getCurrentWindow()
        .isMaximized()
        .then((value) => {
          if (alive) setMaximized(value);
        })
        .catch(() => {});
    };
    refresh();
    void getCurrentWindow()
      .onResized(refresh)
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [visible]);
  if (!visible) return null;
  return (
    <div className="window-controls">
      <button
        className="window-control"
        title="最小化"
        aria-label="最小化"
        onClick={() => void getCurrentWindow().minimize()}
      >
        <Minus size={14} aria-hidden="true" />
      </button>
      <button
        className="window-control"
        title={maximized ? "还原" : "最大化"}
        aria-label={maximized ? "还原" : "最大化"}
        onClick={() => void getCurrentWindow().toggleMaximize()}
      >
        {maximized ? (
          <Copy size={12} aria-hidden="true" />
        ) : (
          <Square size={12} aria-hidden="true" />
        )}
      </button>
      <button
        className="window-control window-control-close"
        title="关闭"
        aria-label="关闭"
        onClick={() => void getCurrentWindow().close()}
      >
        <X size={14} aria-hidden="true" />
      </button>
    </div>
  );
}
