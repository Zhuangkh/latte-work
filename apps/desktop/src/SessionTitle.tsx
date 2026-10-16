import { useLayoutEffect, useRef } from "react";

/** Measure only the hovered row; the browser animates without per-frame JS. */
export function SessionTitle({ title }: { title: string }) {
  const viewport = useRef<HTMLSpanElement>(null);
  const text = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const clip = viewport.current;
    const content = text.current;
    const row = clip?.closest("button");
    if (!clip || !content || !row || typeof content.animate !== "function")
      return;
    let animation: Animation | undefined;
    let observer: ResizeObserver | undefined;
    let motion: MediaQueryList | undefined;
    let previousDistance = 0;
    const reset = () => {
      animation?.cancel();
      animation = undefined;
      delete clip.dataset.scrolling;
    };
    const measure = () => {
      if (!motion) return;
      const distance = Math.ceil(
        content.getBoundingClientRect().width - clip.clientWidth,
      );
      if (motion.matches || clip.clientWidth === 0 || distance <= 1) {
        reset();
        return;
      }
      if (animation && previousDistance === distance) return;
      reset();
      previousDistance = distance;
      clip.dataset.scrolling = "true";
      const travel = (distance / 18) * 1000;
      const duration = 700 + travel + 1200;
      const end = `translateX(-${distance}px)`;
      animation = content.animate(
        [
          { transform: "translateX(0)", offset: 0 },
          { transform: "translateX(0)", offset: 700 / duration },
          { transform: end, offset: (700 + travel) / duration },
          { transform: end, offset: 1 },
        ],
        { duration, iterations: Infinity, easing: "linear" },
      );
    };
    const stop = () => {
      observer?.disconnect();
      observer = undefined;
      motion?.removeEventListener("change", measure);
      motion = undefined;
      window.removeEventListener("blur", stop);
      document.removeEventListener("visibilitychange", hide);
      reset();
    };
    const start = (event?: PointerEvent) => {
      if (event?.pointerType === "touch") return;
      stop();
      motion = window.matchMedia("(prefers-reduced-motion: reduce)");
      motion.addEventListener("change", measure);
      observer = new ResizeObserver(measure);
      observer.observe(clip);
      observer.observe(content);
      window.addEventListener("blur", stop);
      document.addEventListener("visibilitychange", hide);
      measure();
    };
    const hide = () => {
      if (document.hidden) stop();
    };
    const key = (event: KeyboardEvent) => {
      if (
        event.key === "ContextMenu" ||
        event.key === "Escape" ||
        (event.shiftKey && event.key === "F10")
      )
        stop();
    };
    row.addEventListener("pointerenter", start);
    row.addEventListener("pointerleave", stop);
    row.addEventListener("pointerdown", stop);
    row.addEventListener("contextmenu", stop);
    row.addEventListener("keydown", key);
    if (row.matches(":hover")) start();
    return () => {
      stop();
      row.removeEventListener("pointerenter", start);
      row.removeEventListener("pointerleave", stop);
      row.removeEventListener("pointerdown", stop);
      row.removeEventListener("contextmenu", stop);
      row.removeEventListener("keydown", key);
    };
  }, [title]);
  return (
    <span className="session-title-viewport" ref={viewport}>
      <span className="session-title-text" ref={text}>
        {title}
      </span>
    </span>
  );
}
