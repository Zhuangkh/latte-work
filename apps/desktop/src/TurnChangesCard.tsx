import { useEffect, useRef, useState } from "react";
import { request, message } from "./api";
import { SquarePlus, Undo2 } from "lucide-react";
import type { TurnChanges } from "./protocol";
import { useStatusIssues } from "./statusNotices";

export function TurnChangesCard({
  changes: saved,
  open,
  hostId,
  sessionId,
  canUndo,
}: {
  changes: TurnChanges;
  hostId: string;
  sessionId: string;
  canUndo: boolean;
  open: (path?: string) => void;
}) {
  const [changes, setChanges] = useState(saved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useStatusIssues([
    {
      id: `turn-undo:${hostId}:${sessionId}:${saved.request_id}`,
      title: "撤销本轮修改未完成",
      error: changes.undo === "reverted" ? "" : error,
      pending: busy,
      action: { label: "查看变更", run: () => open() },
    },
  ]);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => setChanges(saved), [saved]);
  const { summary } = changes;
  const undoable =
    canUndo &&
    changes.undo === "ready" &&
    !changes.undo_unavailable &&
    summary.entries.length > 0 &&
    !summary.truncated &&
    !summary.unavailable &&
    !changes.background_pending;
  async function undo() {
    if (!undoable || busy) return;
    setBusy(true);
    setError("");
    try {
      const response = await request(hostId, {
        method: "undo_turn_changes",
        session_id: sessionId,
        request_id: changes.request_id,
      });
      if (response.kind !== "turn_changes")
        throw new Error("Server 不支持按轮次撤销，请更新后重试");
      if (mounted.current) setChanges(response.changes);
    } catch (cause) {
      if (mounted.current) setError(message(cause));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }
  if (!summary.entries.length && !changes.undo_unavailable) return null;
  let heading = `已编辑 ${summary.entries.length} 个文件`;
  if (summary.unavailable) heading = "本轮变更未能记录";
  else if (!summary.entries.length) heading = "本轮修改无法撤销";
  return (
    <section className="turn-changes-card" aria-label="本轮文件变更">
      <header>
        <span className="turn-changes-icon">
          <SquarePlus size={22} aria-hidden="true" />
        </span>
        <div className="turn-changes-heading">
          <strong>{heading}</strong>
          {summary.entries.length > 0 && !summary.unavailable && (
            <span className="change-totals">
              <b>+{summary.added}</b>
              <em>-{summary.removed}</em>
            </span>
          )}
        </div>
        {!summary.unavailable && (
          <button
            className="turn-changes-undo"
            disabled={!undoable || busy}
            title={
              undoable
                ? "撤销此轮文件修改；文件已发生后续变化时不会覆盖"
                : changes.undo === "reverted"
                  ? "本轮文件修改已撤销"
                  : changes.undo === "unknown"
                    ? "撤销未能确认，请核对工作区"
                    : changes.undo_unavailable ||
                      "仅可在本轮结束后撤销最近一轮的完整快照"
            }
            onClick={() => void undo()}
          >
            {busy ? (
              "撤销中…"
            ) : changes.undo === "reverted" ? (
              "已撤销"
            ) : changes.undo === "unknown" ? (
              "状态待核对"
            ) : (
              <>
                撤销 <Undo2 size={14} aria-hidden="true" />
              </>
            )}
          </button>
        )}
        {!summary.unavailable && (
          <button className="secondary small" onClick={() => open()}>
            查看变更
          </button>
        )}
      </header>
      {summary.entries.slice(0, 8).map((entry) => (
        <button
          className="turn-changes-file"
          key={entry.path}
          onClick={() => open(entry.path)}
          title={entry.path}
        >
          <span>{entry.path}</span>
          <span className="change-totals">
            {entry.added === null ? (
              "二进制"
            ) : (
              <>
                <b>+{entry.added}</b>
                <em>-{entry.removed}</em>
              </>
            )}
          </span>
        </button>
      ))}
      {summary.entries.length > 8 && (
        <button className="turn-changes-more" onClick={() => open()}>
          查看其余 {summary.entries.length - 8} 个文件
        </button>
      )}
      {(summary.unavailable ||
        summary.truncated ||
        changes.interrupted ||
        changes.undo_unavailable ||
        changes.background_pending) && (
        <p className="turn-changes-note">
          {summary.unavailable ||
            [
              changes.undo_unavailable,
              changes.interrupted && "本轮中断，保留已记录的修改",
              changes.background_pending &&
                "采集快照时后台任务尚未结束，此处保留当时的修改",
              summary.truncated && "部分文件超出采集范围",
            ]
              .filter(Boolean)
              .join(" · ")}
        </p>
      )}
    </section>
  );
}
