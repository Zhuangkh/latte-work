import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown, Bot, ChevronRight } from "lucide-react";
import { MessageContent } from "./MessageContent";
import { WorkingStatus } from "./WorkingStatus";
import type { Subagent, SubagentStatus } from "./protocol";
import type { SubagentsState } from "./useSubagents";
import { useSubagentTranscript } from "./useSubagentTranscript";
import { SubagentTranscript } from "./SubagentTranscript";
import { subagentExecutionStatus } from "./executionStatus";
import { useStatusIssues } from "./statusNotices";

export const subagentLabels: Record<SubagentStatus, string> = {
  running: "处理中",
  paused: "已暂停",
  completed: "已完成",
  failed: "执行失败",
  stopped: "已停止",
  unknown: "状态待确认",
};
export function subagentCounts(tasks: Subagent[]) {
  return {
    active: tasks.filter(
      (task) => task.status === "running" || task.status === "paused",
    ).length,
    completed: tasks.filter((task) => task.status === "completed").length,
    other: tasks.filter(
      (task) => !["running", "paused", "completed"].includes(task.status),
    ).length,
  };
}
export function SubagentsPanel({
  state,
  onOpen,
  taskId,
  hostId = "",
  sessionId,
  active = false,
  connected = true,
}: {
  state: SubagentsState;
  onOpen?: (task: Subagent) => void;
  taskId?: string;
  hostId?: string;
  sessionId?: string;
  active?: boolean;
  connected?: boolean;
}) {
  const [now, setNow] = useState(Date.now());
  const counts = subagentCounts(state.tasks);
  useEffect(() => {
    if (!active || !counts.active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [counts.active, active]);
  const selected = state.tasks.find((task) => task.id === taskId);
  const transcript = useSubagentTranscript(
    hostId,
    sessionId,
    taskId,
    active && connected && !!selected,
    !!selected && ["running", "paused"].includes(selected.status),
    selected?.updated_at,
    state.connectionVersion,
  );
  const root = useRef<HTMLDivElement>(null);
  const readingScope = JSON.stringify([hostId, sessionId, taskId]);
  const position = useRef({ scope: readingScope, top: 0, follow: true });
  const previousRoot = useRef<HTMLDivElement | null>(null);
  const wasActive = useRef(false);
  const [away, setAway] = useState(false);
  useLayoutEffect(() => {
    if (position.current.scope !== readingScope)
      position.current = { scope: readingScope, top: 0, follow: true };
    setAway(!position.current.follow);
  }, [readingScope]);
  useLayoutEffect(() => {
    const el = root.current;
    if (!active) {
      wasActive.current = false;
      return;
    }
    if (!selected || !el) return;
    if (position.current.follow) el.scrollTop = el.scrollHeight;
    else if (previousRoot.current !== el || !wasActive.current)
      el.scrollTop = position.current.top;
    wasActive.current = true;
    previousRoot.current = el;
  }, [transcript.events, selected?.id, active, selected?.status, readingScope]);
  useStatusIssues(
    active && selected
      ? [
          {
            id: `subagent-transcript:${hostId}:${sessionId}:${taskId}`,
            title: `${selected.title} 的过程无法读取`,
            error: transcript.error,
            pending: transcript.loading,
            action: { label: "重试", run: transcript.reload },
          },
        ]
      : [],
  );
  const age = (task: Subagent) => {
    if (task.started_at === null) return "";
    const end = ["running", "paused"].includes(task.status)
      ? now
      : task.updated_at;
    return `${Math.max(0, Math.floor((end - task.started_at) / 1000))}s`;
  };
  if (selected) {
    const working = subagentExecutionStatus(
      selected,
      sessionId ?? "",
      transcript.events,
      connected,
    );
    return (
      <div
        className="conversation subagent-conversation"
        aria-label={`${selected.title} 的对话`}
      >
        <div className="conversation-history">
          <div
            className="conversation-scroll"
            ref={root}
            onScroll={(e) => {
              const el = e.currentTarget;
              if (!active || !el.clientHeight) return;
              position.current = {
                scope: readingScope,
                top: el.scrollTop,
                follow: el.scrollHeight - el.clientHeight - el.scrollTop < 90,
              };
              setAway(!position.current.follow);
            }}
          >
            <div className="messages">
              {sessionId ? (
                <SubagentTranscript
                  task={selected}
                  events={transcript.events}
                  elapsed={age(selected)}
                  incomplete={transcript.incomplete}
                  readingMore={transcript.readingMore}
                />
              ) : selected.summary ? (
                <article className="markdown subagent-result">
                  <MessageContent text={selected.summary} />
                </article>
              ) : !working ? (
                <p className="subagent-empty">尚未上报结果</p>
              ) : null}
              {!selected.summary &&
                ["failed", "stopped"].includes(selected.status) && (
                  <div
                    className={`notice ${selected.status === "failed" ? "failure" : ""}`}
                  >
                    {subagentLabels[selected.status]}
                  </div>
                )}
              {working && (
                <WorkingStatus
                  label={working}
                  animated={connected && selected.status === "running"}
                />
              )}
            </div>
          </div>
        </div>
        {away && (
          <button
            className="return-latest"
            aria-label="回到最新"
            title="回到最新"
            onClick={() => {
              position.current.follow = true;
              setAway(false);
              root.current?.scrollTo({ top: root.current.scrollHeight });
            }}
          >
            <ArrowDown size={15} />
            <span>回到最新</span>
          </button>
        )}
      </div>
    );
  }
  if (taskId)
    return (
      <div className="subagents-panel">
        <p className="subagent-empty">
          {state.loading
            ? "正在读取子智能体…"
            : state.error
              ? null
              : "此子智能体记录暂不可用"}
        </p>
      </div>
    );
  return (
    <div className="subagents-panel">
      {state.loading && !state.tasks.length && (
        <WorkingStatus label="正在读取子智能体…" animated />
      )}
      {!state.loading && !state.error && !state.tasks.length && (
        <p className="subagent-empty">当前对话尚未启动子智能体</p>
      )}
      {(
        [
          [
            "已开启",
            state.tasks.filter((task) =>
              ["running", "paused"].includes(task.status),
            ),
          ],
          ["完成", state.tasks.filter((task) => task.status === "completed")],
          [
            "其他状态",
            state.tasks.filter(
              (task) =>
                !["running", "paused", "completed"].includes(task.status),
            ),
          ],
        ] as const
      ).map(
        ([label, tasks]) =>
          tasks.length > 0 && (
            <section key={label} aria-label={label}>
              <h2 className="subagent-group-label">
                {label} · {tasks.length}
              </h2>
              {tasks.map((task) => (
                <button
                  key={task.id}
                  className="subagent-row"
                  onClick={() => onOpen?.(task)}
                >
                  <Bot size={22} className={`subagent-state ${task.status}`} />
                  <span className="subagent-row-content">
                    <span className="subagent-title" title={task.title}>
                      {task.title}
                    </span>
                    <span className={`subagent-state ${task.status}`}>
                      {subagentLabels[task.status]}
                      {task.last_tool && ` · ${task.last_tool}`}
                    </span>
                  </span>
                  <span className="subagent-age">{age(task)}</span>
                </button>
              ))}
            </section>
          ),
      )}
      {state.truncated && (
        <p className="subagent-empty">仅显示最近 128 项，运行中任务优先。</p>
      )}
    </div>
  );
}
