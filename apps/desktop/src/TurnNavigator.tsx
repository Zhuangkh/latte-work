import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import { ChevronUp, LoaderCircle } from "lucide-react";
import type { TranscriptTurn } from "./TurnTranscript";
import { referenceMessage } from "./referenceMessage";

export function turnNavigationEntries(turns: TranscriptTurn[]) {
  return turns.flatMap((turn) => {
    // A history page can start halfway through a turn. It has no known prompt
    // and must not acquire a fabricated navigation marker.
    if (turn.user?.type !== "user") return [];
    const content = referenceMessage(turn.user.text);
    const reply = [...turn.visible]
      .reverse()
      .find((item) => item.type === "assistant");
    return [
      {
        key: turn.key,
        prompt: excerpt(
          content.text ||
            content.references.map((reference) => reference.name).join("、"),
          240,
        ),
        reply: reply?.type === "assistant" ? excerpt(reply.text, 600) : "",
        status:
          turn.finalKey !== undefined
            ? ""
            : turn.status === "stopped"
              ? "已停止"
              : turn.status === "failed"
                ? "回复中断"
                : turn.status === "unknown"
                  ? "状态待确认"
                  : turn.status === "waiting"
                    ? "等待你的回应"
                    : turn.status === "completed"
                      ? "本轮已结束"
                      : "正在回复…",
      },
    ];
  });
}

function excerpt(text: string, limit: number) {
  const plain = text.replace(/\s+/g, " ").trim();
  return plain.length > limit
    ? `${plain.slice(0, limit).replace(/[\uD800-\uDBFF]$/, "")}…`
    : plain;
}

type Entry = ReturnType<typeof turnNavigationEntries>[number];
type Anchor = { key: string; top: number };

/** Read-only navigation over the same turns rendered by the transcript. */
export function TurnNavigator({
  entries,
  scroller,
  onNavigate,
  hasEarlierHistory,
  earlierHistoryLoading,
  onLoadEarlier,
}: {
  entries: Entry[];
  scroller: RefObject<HTMLDivElement | null>;
  onNavigate: (top: number) => void;
  hasEarlierHistory: boolean;
  earlierHistoryLoading: boolean;
  onLoadEarlier?: () => void;
}) {
  const rail = useRef<HTMLElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const preview = useRef<HTMLDivElement>(null);
  const anchors = useRef<Anchor[]>([]);
  const navigation = useRef<{ key: string; top: number } | null>(null);
  const [activeKey, setActiveKey] = useState<string>();
  const [focusKey, setFocusKey] = useState<string>();
  const [previewKey, setPreviewKey] = useState<string>();
  const [previewTop, setPreviewTop] = useState(0);
  const tooltipId = useId();
  const entry = entries.find((item) => item.key === previewKey);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    let frame = 0;
    let needsMeasure = true;
    const keys = new Set(entries.map((entry) => entry.key));
    const update = () => {
      frame = 0;
      if (needsMeasure) {
        const top = el.getBoundingClientRect().top;
        anchors.current = [
          ...el.querySelectorAll<HTMLElement>("[data-turn-key]"),
        ]
          .filter((node) => keys.has(node.dataset.turnKey!))
          .map((node) => ({
            key: node.dataset.turnKey!,
            top: el.scrollTop + node.getBoundingClientRect().top - top,
          }));
        needsMeasure = false;
      }
      const known = anchors.current;
      if (!known.length) return;
      const locked = navigation.current;
      if (
        locked &&
        keys.has(locked.key) &&
        Math.abs(el.scrollTop - locked.top) < 2
      ) {
        setActiveKey(locked.key);
        return;
      }
      navigation.current = null;
      if (
        el.scrollTop > 0 &&
        el.scrollHeight - el.clientHeight - el.scrollTop <= 2
      ) {
        setActiveKey(known.at(-1)!.key);
        return;
      }
      const readingTop = el.scrollTop + Math.min(100, el.clientHeight * 0.2);
      let low = 0,
        high = known.length;
      while (low < high) {
        const middle = (low + high) >>> 1;
        if (known[middle].top <= readingTop) low = middle + 1;
        else high = middle;
      }
      setActiveKey(known[Math.max(0, low - 1)].key);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    const measure = () => {
      needsMeasure = true;
      schedule();
    };
    const release = () => {
      navigation.current = null;
      schedule();
    };
    schedule();
    el.addEventListener("scroll", schedule, { passive: true });
    el.addEventListener("wheel", release, { passive: true });
    window.addEventListener("resize", measure);
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(measure);
    observer?.observe(el);
    const messages = el.querySelector(".messages");
    if (messages) observer?.observe(messages);
    return () => {
      cancelAnimationFrame(frame);
      el.removeEventListener("scroll", schedule);
      el.removeEventListener("wheel", release);
      window.removeEventListener("resize", measure);
      observer?.disconnect();
    };
  }, [entries, scroller]);

  useLayoutEffect(() => {
    // Keep the current marker visible in very long conversations without moving
    // the transcript or scrolling an ancestor of the rail.
    const el = list.current;
    const button = focusKey
      ? [...(el?.querySelectorAll<HTMLElement>("[data-jump-key]") ?? [])].find(
          (node) => node.dataset.jumpKey === focusKey,
        )
      : el?.querySelector<HTMLElement>('[aria-current="location"]');
    if (!el || !button) return;
    if (button.offsetTop < el.scrollTop) el.scrollTop = button.offsetTop;
    else if (
      button.offsetTop + button.offsetHeight >
      el.scrollTop + el.clientHeight
    )
      el.scrollTop = button.offsetTop + button.offsetHeight - el.clientHeight;
  }, [activeKey, focusKey, entries]);

  useLayoutEffect(() => {
    if (!entry || !rail.current || !preview.current) return;
    const button = [
      ...rail.current.querySelectorAll<HTMLElement>("[data-jump-key]"),
    ].find((node) => node.dataset.jumpKey === entry.key);
    if (!button) return;
    const place = () => {
      if (!rail.current || !preview.current) return;
      const railRect = rail.current.getBoundingClientRect();
      const buttonRect = button.getBoundingClientRect();
      setPreviewTop(
        Math.max(
          0,
          Math.min(
            buttonRect.top -
              railRect.top +
              buttonRect.height / 2 -
              preview.current.offsetHeight / 2,
            railRect.height - preview.current.offsetHeight,
          ),
        ),
      );
    };
    place();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    observer?.observe(rail.current);
    observer?.observe(preview.current);
    window.addEventListener("resize", place);
    const markers = list.current;
    markers?.addEventListener("scroll", place, { passive: true });
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", place);
      markers?.removeEventListener("scroll", place);
    };
  }, [entry, activeKey]);

  function jump(key: string) {
    const el = scroller.current;
    const node =
      el &&
      [...el.querySelectorAll<HTMLElement>("[data-turn-key]")].find(
        (node) => node.dataset.turnKey === key,
      );
    if (!el || !node) return;
    const top = Math.max(
      0,
      Math.min(
        el.scrollTop +
          node.getBoundingClientRect().top -
          el.getBoundingClientRect().top -
          16,
        el.scrollHeight - el.clientHeight,
      ),
    );
    navigation.current = { key, top };
    setActiveKey(key);
    setPreviewKey(undefined);
    onNavigate(top);
  }

  return (
    <nav className="turn-navigator" aria-label="对话轮次" ref={rail}>
      {hasEarlierHistory && onLoadEarlier && (
        <button
          className="turn-navigator-earlier"
          aria-label={
            earlierHistoryLoading ? "正在加载更早的轮次" : "加载更早的轮次"
          }
          title="加载更早的轮次"
          disabled={earlierHistoryLoading}
          onClick={onLoadEarlier}
        >
          {earlierHistoryLoading ? (
            <LoaderCircle size={14} className="spin" />
          ) : (
            <ChevronUp size={14} />
          )}
        </button>
      )}
      <div
        className="turn-navigator-list"
        ref={list}
        onMouseLeave={() => setPreviewKey(undefined)}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget)) {
            setFocusKey(undefined);
            setPreviewKey(undefined);
          }
        }}
      >
        {entries.map((item, index) => (
          <button
            key={item.key}
            data-jump-key={item.key}
            aria-label={`跳转到：${excerpt(item.prompt, 80)}`}
            aria-current={activeKey === item.key ? "location" : undefined}
            aria-describedby={previewKey === item.key ? tooltipId : undefined}
            tabIndex={
              (focusKey ?? activeKey ?? entries.at(-1)?.key) === item.key
                ? 0
                : -1
            }
            onMouseEnter={() => setPreviewKey(item.key)}
            onFocus={() => {
              setFocusKey(item.key);
              setPreviewKey(item.key);
            }}
            onClick={() => jump(item.key)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setPreviewKey(undefined);
                return;
              }
              const next =
                event.key === "ArrowUp"
                  ? Math.max(0, index - 1)
                  : event.key === "ArrowDown"
                    ? Math.min(entries.length - 1, index + 1)
                    : event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? entries.length - 1
                        : null;
              if (next === null) return;
              event.preventDefault();
              const buttons =
                list.current?.querySelectorAll<HTMLButtonElement>("button");
              buttons?.[next]?.focus();
            }}
          >
            <span aria-hidden="true" />
          </button>
        ))}
      </div>
      {entry && (
        <div
          className="turn-navigator-preview"
          ref={preview}
          role="tooltip"
          id={tooltipId}
          style={{ top: previewTop }}
        >
          <p className="turn-navigator-prompt">{entry.prompt}</p>
          {entry.status && <small>{entry.status}</small>}
          <p className="turn-navigator-reply">
            {entry.reply || (entry.status ? "尚无文字回复" : "暂无文字回复")}
          </p>
        </div>
      )}
    </nav>
  );
}
