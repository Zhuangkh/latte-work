import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
} from "react";
import type { HostedProject } from "./projectCatalog";
import { projectOrderKey, moveProjectKeys } from "./projectOrder";

type Target = {
  key: string;
  after: boolean;
  top: number;
  left: number;
  width: number;
};
type Drag = {
  project: HostedProject;
  x: number;
  y: number;
  width: number;
  target: Target | null;
};
type Move = (source: string, target: string, after: boolean) => void;

/** Pointer capture works in the native WebView without HTML drag-and-drop. */
export function useProjectReorder({
  projects,
  enabled,
  onMove,
}: {
  projects: HostedProject[];
  enabled: boolean;
  onMove: Move;
}) {
  const list = useRef<HTMLDivElement>(null);
  const current = useRef({ projects, enabled, onMove });
  current.current = { projects, enabled, onMove };
  const cancel = useRef<(() => void) | undefined>(undefined);
  const source = useRef<string | null>(null);
  const suppressClick = useRef(false);
  const clickTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const [drag, setDrag] = useState<Drag | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const projectKeys = JSON.stringify(projects.map(projectOrderKey));
  useEffect(() => {
    if (
      !enabled ||
      (source.current &&
        !projects.some((p) => projectOrderKey(p) === source.current))
    )
      cancel.current?.();
  }, [enabled, projectKeys]);
  useEffect(
    () => () => {
      cancel.current?.();
      clearTimeout(clickTimer.current);
    },
    [],
  );

  function move(key: string, target: Target) {
    const projects = current.current.projects;
    const keys = projects.map(projectOrderKey);
    const next = moveProjectKeys(keys, key, target.key, target.after);
    if (next === keys) return;
    current.current.onMove(key, target.key, target.after);
    const name = projects.find((p) => projectOrderKey(p) === key)?.name;
    setAnnouncement(`${name} 已移至第 ${next.indexOf(key) + 1} 个位置`);
  }

  function start(
    event: PointerEvent<HTMLButtonElement>,
    project: HostedProject,
  ) {
    if (
      !current.current.enabled ||
      current.current.projects.length < 2 ||
      event.button !== 0 ||
      (event.pointerType && event.pointerType !== "mouse") ||
      event.isPrimary === false
    )
      return;
    cancel.current?.();
    clearTimeout(clickTimer.current);
    suppressClick.current = false;
    const key = projectOrderKey(project);
    const button = event.currentTarget;
    const origin = { x: event.clientX, y: event.clientY };
    let point = origin;
    let active = false;
    let finished = false;
    let frame = 0;
    let target: Target | null = null;
    const rect = button.getBoundingClientRect();
    const width = Math.min(240, Math.max(120, rect.width - 24));
    const pointerId = event.pointerId;
    source.current = key;

    function paint() {
      const element = list.current;
      const nav = element?.closest<HTMLElement>(".sidebar-navigation");
      target = null;
      if (!element || !nav) return;
      const area = element.getBoundingClientRect();
      const viewport = nav.getBoundingClientRect();
      const top = Math.max(area.top, viewport.top);
      const bottom = Math.min(area.bottom, viewport.bottom);
      if (
        point.x >= area.left &&
        point.x <= area.right &&
        point.y >= top - 8 &&
        point.y <= bottom + 8
      ) {
        const rows = [
          ...element.querySelectorAll<HTMLElement>(
            ".project-group[data-project-key]",
          ),
        ].filter((row) => row.dataset.projectKey !== key);
        const row = rows.find((row) => {
          const box = row.getBoundingClientRect();
          return point.y < box.top + box.height / 2;
        });
        const anchor = row ?? rows.at(-1);
        if (anchor) {
          const box = anchor.getBoundingClientRect();
          const line = row ? box.top : box.bottom;
          if (line >= viewport.top && line <= viewport.bottom)
            target = {
              key: anchor.dataset.projectKey!,
              after: !row,
              top: line,
              left: area.left + 4,
              width: Math.max(0, area.width - 8),
            };
        }
      }
      setDrag({
        project,
        width,
        x: Math.max(4, Math.min(window.innerWidth - width - 4, point.x + 12)),
        y: Math.max(4, Math.min(window.innerHeight - 42, point.y + 12)),
        target,
      });
    }
    function scroll() {
      frame = 0;
      if (!active || finished) return;
      const nav = list.current?.closest<HTMLElement>(".sidebar-navigation");
      if (!nav) return;
      const box = nav.getBoundingClientRect();
      if (
        point.x < box.left ||
        point.x > box.right ||
        point.y < box.top - 8 ||
        point.y > box.bottom + 8
      )
        return;
      const amount =
        point.y < box.top + 28 ? -8 : point.y > box.bottom - 28 ? 8 : 0;
      if (!amount) return;
      const previous = nav.scrollTop;
      nav.scrollTop += amount;
      if (previous !== nav.scrollTop) {
        paint();
        frame = requestAnimationFrame(scroll);
      }
    }
    function pointerMove(e: globalThis.PointerEvent) {
      if (e.pointerId !== pointerId || finished) return;
      if ((e.buttons & 1) === 0) {
        finish(false);
        return;
      }
      point = { x: e.clientX, y: e.clientY };
      if (!active && Math.hypot(point.x - origin.x, point.y - origin.y) < 5)
        return;
      if (!active) {
        active = true;
        button.setPointerCapture?.(pointerId);
      }
      e.preventDefault();
      paint();
      if (!frame) frame = requestAnimationFrame(scroll);
    }
    function finish(commit: boolean) {
      if (finished) return;
      finished = true;
      cancelAnimationFrame(frame);
      window.removeEventListener("pointermove", pointerMove);
      window.removeEventListener("pointerup", pointerUp);
      window.removeEventListener("pointercancel", pointerCancel);
      window.removeEventListener("keydown", keyDown, true);
      window.removeEventListener("blur", pointerCancel);
      button.removeEventListener("lostpointercapture", pointerCancel);
      source.current = null;
      cancel.current = undefined;
      if (active) {
        suppressClick.current = true;
        clickTimer.current = setTimeout(() => {
          suppressClick.current = false;
        }, 0);
        if (commit && target) move(key, target);
        setDrag(null);
      }
      if (button.hasPointerCapture?.(pointerId))
        button.releasePointerCapture(pointerId);
    }
    function pointerUp(e: globalThis.PointerEvent) {
      if (e.pointerId !== pointerId) return;
      point = { x: e.clientX, y: e.clientY };
      if (active) paint();
      finish(true);
    }
    function pointerCancel() {
      finish(false);
    }
    function keyDown(e: globalThis.KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      finish(false);
    }
    cancel.current = pointerCancel;
    window.addEventListener("pointermove", pointerMove, { passive: false });
    window.addEventListener("pointerup", pointerUp);
    window.addEventListener("pointercancel", pointerCancel);
    window.addEventListener("keydown", keyDown, true);
    window.addEventListener("blur", pointerCancel);
    button.addEventListener("lostpointercapture", pointerCancel);
  }
  function keyboard(
    event: KeyboardEvent<HTMLButtonElement>,
    project: HostedProject,
  ) {
    if (
      !enabled ||
      !event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      !["ArrowUp", "ArrowDown"].includes(event.key)
    )
      return;
    const keys = projects.map(projectOrderKey);
    const key = projectOrderKey(project);
    const index = keys.indexOf(key);
    const down = event.key === "ArrowDown";
    const target = keys[index + (down ? 1 : -1)];
    event.preventDefault();
    if (!target) return;
    move(key, { key: target, after: down, top: 0, left: 0, width: 0 });
    const button = event.currentTarget;
    queueMicrotask(() => {
      if (button.isConnected) button.focus();
    });
  }
  function click(event: React.MouseEvent) {
    if (!drag && !suppressClick.current) return;
    event.preventDefault();
    event.stopPropagation();
    suppressClick.current = false;
  }
  return { list, drag, announcement, start, keyboard, click };
}
