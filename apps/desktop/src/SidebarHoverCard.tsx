import {
  cloneElement,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentProps,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { Folder } from "lucide-react";
import {
  agentSessionLabels,
  type AgentSessionState,
} from "./agentSessionState";

/** Read-only sidebar information; hovering never requests or opens an Agent. */
export function HoverCard({
  content,
  children,
  disabled = false,
}: {
  content: ReactNode;
  children: ReactElement<ComponentProps<"button">>;
  disabled?: boolean;
}) {
  const id = useId();
  const anchor = useRef<HTMLDivElement>(null);
  const card = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 0, top: 0 });
  function cancelTimer() {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }
  function show() {
    if (disabled) return;
    cancelTimer();
    timer.current = setTimeout(() => setOpen(true), 350);
  }
  function dismiss() {
    cancelTimer();
    setOpen(false);
  }
  function leave() {
    cancelTimer();
    timer.current = setTimeout(() => setOpen(false), 100);
  }
  useEffect(() => cancelTimer, []);
  useEffect(() => {
    if (disabled) dismiss();
  }, [disabled]);
  useLayoutEffect(() => {
    if (!open || !anchor.current || !card.current) return;
    const box = anchor.current.getBoundingClientRect();
    const tooltip = card.current.getBoundingClientRect();
    const right = box.right + 6;
    const preferred =
      right + tooltip.width <= window.innerWidth - 8
        ? right
        : box.left - tooltip.width - 6;
    setPosition({
      left: Math.max(
        8,
        Math.min(preferred, window.innerWidth - tooltip.width - 8),
      ),
      top: Math.max(
        8,
        Math.min(box.top, window.innerHeight - tooltip.height - 8),
      ),
    });
  }, [open, content]);
  useEffect(() => {
    if (!open) return;
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") dismiss();
    };
    const scroll = (event: Event) => {
      if (
        !(event.target instanceof Node) ||
        !card.current?.contains(event.target)
      )
        dismiss();
    };
    window.addEventListener("keydown", key);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", scroll, true);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", scroll, true);
    };
  }, [open]);
  return (
    <div
      ref={anchor}
      className="sidebar-hover-anchor"
      onMouseEnter={show}
      onMouseLeave={leave}
      onFocus={show}
      onBlur={dismiss}
      onClick={dismiss}
      onPointerDown={dismiss}
      onContextMenu={dismiss}
      onKeyDown={(event) => {
        if (
          ["Escape", "Enter", " ", "ContextMenu"].includes(event.key) ||
          (event.shiftKey && event.key === "F10")
        )
          dismiss();
      }}
    >
      {cloneElement(children, { "aria-describedby": open ? id : undefined })}
      {open &&
        createPortal(
          <div
            ref={card}
            id={id}
            role="tooltip"
            className="sidebar-hover-card"
            style={position}
            onMouseEnter={cancelTimer}
            onMouseLeave={leave}
          >
            {content}
          </div>,
          document.body,
        )}
    </div>
  );
}

export function SessionHoverCard({
  title,
  project,
  host,
  taskStatus,
  agentState,
  children,
}: {
  title: string;
  project: string;
  host: string;
  taskStatus: string;
  agentState: AgentSessionState;
  children: ReactElement<ComponentProps<"button">>;
}) {
  const sessionStatus =
    agentState === "open"
      ? "已打开"
      : agentState === "closed"
        ? null
        : agentSessionLabels[agentState];
  return (
    <HoverCard
      content={
        <>
          <div className="sidebar-hover-title">{title}</div>
          <div className="sidebar-hover-project">
            <Folder size={15} aria-hidden="true" />
            <span>{project}</span>
          </div>
          <div className="sidebar-hover-details">
            <span>
              {host} · {taskStatus}
            </span>
            {sessionStatus && (
              <span className="sidebar-hover-status">{sessionStatus}</span>
            )}
          </div>
        </>
      }
    >
      {children}
    </HoverCard>
  );
}

export function ProjectHoverCard({
  title,
  path,
  host,
  total,
  opened,
  children,
  disabled,
}: {
  title: string;
  path: string;
  host: string;
  total: number | null;
  opened: number | null;
  children: ReactElement<ComponentProps<"button">>;
  disabled?: boolean;
}) {
  return (
    <HoverCard
      disabled={disabled}
      content={
        <>
          <div className="sidebar-hover-title">{title}</div>
          <div className="sidebar-hover-summary">
            {total === null ? "任务数量待确认" : `${total} 个任务`} ·{" "}
            {opened === null ? "会话状态待确认" : `${opened} 个已打开`}
          </div>
          <div className="sidebar-hover-project">
            <Folder size={15} aria-hidden="true" />
            <span>{path}</span>
          </div>
          <div className="sidebar-hover-details">
            <span>{host}</span>
          </div>
        </>
      }
    >
      {children}
    </HoverCard>
  );
}
