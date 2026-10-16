import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  ListEnd,
  Pencil,
  Trash2,
  Play,
  MoreHorizontal,
  ArrowUp,
  GripVertical,
} from "lucide-react";
import { createPortal } from "react-dom";
import { request, message } from "./api";
import type { Request, QueuedMessage } from "./protocol";
import { usePollingSnapshot, LIVE_POLL_MS } from "./usePollingSnapshot";
import { useStatusIssues } from "./statusNotices";
import { referenceMessage } from "./referenceMessage";
const initial = {
  entries: [] as QueuedMessage[],
  paused: false,
  reason: null as string | null,
};

export function MessageQueue({
  hostId,
  sessionId,
  connected,
  active,
  revision,
  running,
  onEdit,
}: {
  hostId: string;
  sessionId?: string;
  connected: boolean;
  active: boolean;
  revision: string;
  running: boolean;
  onEdit?: (entry: QueuedMessage) => void;
}) {
  const scope = JSON.stringify([hostId, sessionId]);
  const context = JSON.stringify([scope, active, connected]);
  const current = useRef({ context, scope, version: 0, mounted: true });
  if (current.current.context !== context)
    current.current = {
      ...current.current,
      context,
      scope,
      version: current.current.version + 1,
    };
  const version = current.current.version;
  const valid = () =>
    current.current.mounted && current.current.version === version;
  useEffect(() => {
    current.current.mounted = true;
    return () => {
      current.current.mounted = false;
    };
  }, []);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [recover, setRecover] =
    useState<Extract<Request, { method: "take_queued_message" }>>();
  const drag = useRef<
    | {
        id: string;
        x: number;
        y: number;
        moved: boolean;
        target?: { id: string; after: boolean };
      }
    | undefined
  >(undefined);
  const [dropTarget, setDropTarget] = useState<{
    id: string;
    after: boolean;
  }>();
  const [dragging, setDragging] = useState<string>();
  const [announcement, setAnnouncement] = useState("");
  const grips = useRef(new Map<string, HTMLButtonElement>());
  const focusAfterReorder = useRef<{ id: string; before: unknown } | undefined>(
    undefined,
  );
  const latestSnapshot = useRef<unknown>(undefined);
  function endDrag() {
    drag.current = undefined;
    setDragging(undefined);
    setDropTarget(undefined);
  }
  useEffect(() => {
    setBusy(false);
    setError("");
    setAnnouncement("");
    focusAfterReorder.current = undefined;
    endDrag();
  }, [context]);
  const read = useCallback(async () => {
    const response = await request(hostId, {
      method: "message_queue",
      session_id: sessionId!,
    });
    if (response.kind !== "message_queue")
      throw new Error("此 Server 不支持消息排队，请更新 Server");
    return response;
  }, [hostId, sessionId]);
  const state = usePollingSnapshot(
    scope,
    active && connected && !!sessionId,
    read,
    initial,
    `${revision}:${reload}:${running}`,
    LIVE_POLL_MS,
    0,
  );
  latestSnapshot.current = state.data;
  useEffect(() => {
    const pending = focusAfterReorder.current;
    if (!busy && pending && state.data !== pending.before) {
      grips.current.get(pending.id)?.focus();
      focusAfterReorder.current = undefined;
    }
  }, [busy, state.data]);
  async function act(command: Request, focusId?: string) {
    if (busy || !connected || !active) return;
    setBusy(true);
    setAnnouncement("");
    try {
      const response = await request(hostId, command);
      const taken =
        command.method === "take_queued_message" &&
        response.kind === "queued_message_taken";
      // A confirmed take belongs to its captured session even after navigation.
      if (taken) onEdit?.(response.entry);
      if (!valid()) return;
      if (!taken && response.kind !== "message_queue") {
        throw new Error("队列操作未确认，请刷新后核对");
      }
      setError("");
      if (command.method === "reorder_message_queue") {
        setAnnouncement("待发送顺序已保存");
        if (focusId)
          focusAfterReorder.current = {
            id: focusId,
            before: latestSnapshot.current,
          };
      }
      if (taken) setRecover(undefined);
      setReload((v) => v + 1);
    } catch (cause) {
      if (
        command.method === "take_queued_message" &&
        current.current.mounted &&
        current.current.scope === scope
      )
        setRecover(command);
      if (valid()) {
        setError(message(cause));
        setReload((v) => v + 1);
      }
    } finally {
      if (valid()) setBusy(false);
    }
  }
  useStatusIssues([
    {
      id: `message-queue:${scope}`,
      title: "消息队列暂不可用",
      error:
        active && connected
          ? error ||
            (recover ? "编辑取回结果待确认，请恢复编辑内容" : state.error)
          : "",
      pending: busy,
      action: {
        label: recover ? "恢复编辑内容" : "刷新队列",
        run: () => {
          if (recover) {
            void act(recover);
            return;
          }
          setError("");
          setReload((v) => v + 1);
        },
      },
    },
  ]);
  const { entries, paused, reason } = state.data;
  if (!entries.length) return null;
  const disabled = busy || !connected || !active;
  const canReorder = entries.length > 1 && !disabled;
  function reorder(
    expected: string[],
    id: string,
    target: string,
    after: boolean,
    keyboard = false,
  ) {
    if (!expected.includes(id) || !expected.includes(target)) {
      setAnnouncement("队列已更新，已取消本次拖动");
      return;
    }
    const order = expected.filter((key) => key !== id);
    const index = order.indexOf(target);
    if (index < 0 || id === target) return;
    order.splice(index + (after ? 1 : 0), 0, id);
    if (order.every((key, i) => key === expected[i])) return;
    void act(
      {
        method: "reorder_message_queue",
        session_id: sessionId!,
        expected,
        order,
      },
      keyboard ? id : undefined,
    );
  }
  return (
    <section className="message-queue" aria-label="待发送消息">
      {paused && (
        <div className="message-queue-heading">
          <ListEnd size={14} />
          <span>待发送 · {entries.length}</span>
          <button
            disabled={disabled}
            onClick={() =>
              void act({
                method: "resume_message_queue",
                session_id: sessionId!,
                provider: null,
              })
            }
          >
            <Play size={14} />
            继续排队
          </button>
        </div>
      )}
      {paused && (
        <p className="message-queue-reason">{reason || "队列已暂停"}</p>
      )}
      <span className="visually-hidden" role="status">
        {announcement}
      </span>
      <ol>
        {entries.map((entry, index) => {
          const content = referenceMessage(entry.text);
          return (
            <li
              key={entry.request_id}
              className={[
                dragging === entry.request_id ? "message-queue-dragging" : "",
                dropTarget?.id === entry.request_id
                  ? dropTarget.after
                    ? "message-queue-drop-after"
                    : "message-queue-drop-before"
                  : "",
              ].join(" ")}
            >
              <>
                {entries.length === 1 && (
                  <ListEnd
                    size={14}
                    className="message-queue-marker"
                    aria-hidden="true"
                  />
                )}
                {entries.length > 1 && (
                  <button
                    ref={(node) => {
                      if (node) grips.current.set(entry.request_id, node);
                      else grips.current.delete(entry.request_id);
                    }}
                    className="icon-button message-queue-grip"
                    aria-label={`排序排队消息：${content.text}`}
                    title="拖动排序，或按上、下方向键调整"
                    disabled={!canReorder}
                    onPointerDown={(event) => {
                      if (!canReorder || event.button !== 0) return;
                      event.preventDefault();
                      event.currentTarget.focus();
                      event.currentTarget.setPointerCapture?.(event.pointerId);
                      drag.current = {
                        id: entry.request_id,
                        x: event.clientX,
                        y: event.clientY,
                        moved: false,
                      };
                      setAnnouncement("");
                    }}
                    onPointerMove={(event) => {
                      const moving = drag.current;
                      if (!canReorder || !moving) return;
                      if (
                        !moving.moved &&
                        Math.hypot(
                          event.clientX - moving.x,
                          event.clientY - moving.y,
                        ) < 4
                      )
                        return;
                      moving.moved = true;
                      setDragging(moving.id);
                      const list = event.currentTarget.closest("ol")!;
                      const bounds = list.getBoundingClientRect();
                      const inside =
                        event.clientX >= bounds.left &&
                        event.clientX <= bounds.right &&
                        event.clientY >= bounds.top &&
                        event.clientY <= bounds.bottom;
                      moving.target = undefined;
                      if (inside) {
                        if (event.clientY < bounds.top + 24)
                          list.scrollTop -= 12;
                        else if (event.clientY > bounds.bottom - 24)
                          list.scrollTop += 12;
                        for (const [i, row] of Array.from(
                          list.children,
                        ).entries()) {
                          const box = row.getBoundingClientRect();
                          if (
                            event.clientY >= box.top &&
                            event.clientY <= box.bottom &&
                            entries[i].request_id !== moving.id
                          ) {
                            moving.target = {
                              id: entries[i].request_id,
                              after: event.clientY > box.top + box.height / 2,
                            };
                            break;
                          }
                        }
                      }
                      setDropTarget(moving.target);
                    }}
                    onPointerUp={(event) => {
                      const moving = drag.current;
                      if (canReorder && moving?.moved && moving.target)
                        reorder(
                          entries.map((item) => item.request_id),
                          moving.id,
                          moving.target.id,
                          moving.target.after,
                        );
                      endDrag();
                      if (
                        event.currentTarget.hasPointerCapture?.(event.pointerId)
                      )
                        event.currentTarget.releasePointerCapture(
                          event.pointerId,
                        );
                    }}
                    onPointerCancel={endDrag}
                    onLostPointerCapture={endDrag}
                    onKeyDown={(event) => {
                      if (event.key === "Escape") {
                        endDrag();
                        return;
                      }
                      if (
                        !canReorder ||
                        !["ArrowUp", "ArrowDown"].includes(event.key)
                      )
                        return;
                      event.preventDefault();
                      const down = event.key === "ArrowDown";
                      const target = entries[index + (down ? 1 : -1)];
                      if (target)
                        reorder(
                          entries.map((item) => item.request_id),
                          entry.request_id,
                          target.request_id,
                          down,
                          true,
                        );
                    }}
                  >
                    <GripVertical size={14} />
                  </button>
                )}
                <span className="message-queue-text" title={content.text}>
                  {content.text}
                  {content.references.length > 0 && (
                    <small> · {content.references.length} 个引用</small>
                  )}
                </span>
                <button
                  className="icon-button"
                  aria-label={`发送排队消息：${content.text}`}
                  title="发送给 Agent（不中断当前任务）"
                  disabled={disabled}
                  onClick={() =>
                    void act({
                      method: "dispatch_queued_message",
                      interrupt: false,
                      session_id: sessionId!,
                      request_id: entry.request_id,
                      revision: entry.revision,
                      provider: null,
                    })
                  }
                >
                  <ArrowUp size={14} />
                </button>
                <button
                  className="icon-button"
                  aria-label={`删除排队消息：${content.text}`}
                  disabled={disabled}
                  onClick={() =>
                    void act({
                      method: "cancel_queued_message",
                      session_id: sessionId!,
                      request_id: entry.request_id,
                      revision: entry.revision,
                    })
                  }
                >
                  <Trash2 size={14} />
                </button>
                <QueueMessageMenu
                  disabled={disabled}
                  canEdit={!!onEdit}
                  stopAndSend={() =>
                    void act({
                      method: "dispatch_queued_message",
                      interrupt: true,
                      session_id: sessionId!,
                      request_id: entry.request_id,
                      revision: entry.revision,
                      provider: null,
                    })
                  }
                  name={content.text}
                  edit={() =>
                    void act({
                      method: "take_queued_message",
                      session_id: sessionId!,
                      request_id: entry.request_id,
                      revision: entry.revision,
                    })
                  }
                />
              </>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

function QueueMessageMenu({
  disabled,
  name,
  edit,
  canEdit,
  stopAndSend,
}: {
  disabled: boolean;
  name: string;
  edit: () => void;
  canEdit: boolean;
  stopAndSend: () => void;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [point, setPoint] = useState<{ x: number; y: number }>();
  useLayoutEffect(() => {
    if (!point || !menu.current) return;
    const box = menu.current.getBoundingClientRect();
    menu.current.style.left = `${Math.max(8, Math.min(point.x - box.width, innerWidth - box.width - 8))}px`;
    menu.current.style.top = `${Math.max(8, Math.min(point.y, innerHeight - box.height - 8))}px`;
    menu.current
      .querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
  }, [point]);
  useEffect(() => {
    if (!point) return;
    const outside = (event: PointerEvent) => {
      if (
        !menu.current?.contains(event.target as Node) &&
        !trigger.current?.contains(event.target as Node)
      )
        setPoint(undefined);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Tab") {
        // Put focus on the persistent trigger before the portal is removed;
        // normal Tab/Shift+Tab then continues from the row instead of body.
        trigger.current?.focus();
        setPoint(undefined);
        return;
      }
      if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        const buttons = Array.from(
          menu.current?.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ) ?? [],
        );
        const index = buttons.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        const next =
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? buttons.length - 1
              : (index + (event.key === "ArrowUp" ? -1 : 1) + buttons.length) %
                buttons.length;
        buttons[next]?.focus();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setPoint(undefined);
        trigger.current?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", key, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", key, true);
    };
  }, [point]);
  return (
    <>
      <button
        ref={trigger}
        className="icon-button"
        aria-label={`排队消息更多操作：${name}`}
        aria-haspopup="menu"
        aria-expanded={!!point}
        disabled={disabled}
        onClick={() => {
          const box = trigger.current!.getBoundingClientRect();
          setPoint(point ? undefined : { x: box.right, y: box.bottom + 4 });
        }}
      >
        <MoreHorizontal size={14} />
      </button>
      {point &&
        !disabled &&
        createPortal(
          <div
            ref={menu}
            role="menu"
            aria-label="排队消息操作"
            className="project-context-menu"
            style={{ position: "fixed", left: point.x, top: point.y }}
          >
            <button
              role="menuitem"
              disabled={!canEdit}
              onClick={() => {
                setPoint(undefined);
                edit();
              }}
            >
              <Pencil size={16} />
              编辑消息
            </button>
            <button
              role="menuitem"
              onClick={() => {
                setPoint(undefined);
                stopAndSend();
              }}
            >
              <ArrowUp size={16} />
              停止当前并发送
            </button>
          </div>,
          document.body,
        )}
    </>
  );
}
