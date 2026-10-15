import type {
  Event,
  ExecutionPhase,
  Session,
  Subagent,
  Status,
} from "./protocol";

/** Derive only from this turn's reported events; silence is never thinking. */
export function executionStatus(
  session: Session | undefined,
  events: Event[],
  connected: boolean,
  stopping = false,
): string | null {
  if (!session || !["running", "waiting"].includes(session.status)) return null;
  return reportedExecutionStatus(
    session.status,
    session.id,
    events,
    connected,
    stopping,
  );
}

export function subagentExecutionStatus(
  task: Subagent,
  sessionId: string,
  events: Event[],
  connected: boolean,
): string | null {
  if (task.status === "unknown")
    return connected ? "状态待确认" : "连接已断开，状态待确认";
  if (task.status === "paused")
    return connected ? "已暂停" : "连接已断开，状态待确认";
  if (task.status !== "running") return null;
  return reportedExecutionStatus("running", sessionId, events, connected);
}

function reportedExecutionStatus(
  status: Status,
  sessionId: string,
  events: Event[],
  connected: boolean,
  stopping = false,
) {
  if (!connected) return "连接已断开，状态待确认";
  if (stopping) return "正在停止…";
  if (status === "waiting") {
    const questions = new Set<string>();
    for (const { event } of events.filter(
      (event) => event.session_id === sessionId,
    )) {
      if (
        event.kind === "user" ||
        (event.kind === "state" &&
          !["running", "waiting"].includes(event.status))
      )
        questions.clear();
      if (event.kind === "question") questions.add(event.request_id);
      if (event.kind === "question_resolved")
        questions.delete(event.request_id);
    }
    return questions.size ? "等待你的回答" : "等待你的确认";
  }
  let phase: ExecutionPhase = "waiting";
  const pending = new Map<string, string>();
  for (const { event } of events.filter((e) => e.session_id === sessionId)) {
    // Events may arrive across a navigation boundary.
    // Caller's session scope must match before consuming them.
    if (
      event.kind === "user" ||
      (event.kind === "state" && !["running", "waiting"].includes(event.status))
    ) {
      pending.clear();
      phase = "waiting";
    } else if (event.kind === "progress") phase = event.phase;
    else if (event.kind === "text") phase = "replying";
    else if (event.kind === "tool") {
      pending.set(event.id, event.name.toLowerCase());
      phase = "waiting";
    } else if (event.kind === "tool_result") {
      pending.delete(event.id);
      phase = "waiting";
    }
  }
  if (phase === "thinking") return "正在思考…";
  if (phase === "replying") return "正在回复…";
  const tools = [...pending.values()];
  if (tools.length > 1) return "正在执行工具…";
  if (tools.length === 1) {
    const labels: Record<string, string> = {
      read: "正在读取文件…",
      bash: "正在运行命令…",
      write: "正在写入文件…",
      edit: "正在编辑文件…",
      multiedit: "正在编辑文件…",
      grep: "正在搜索…",
      glob: "正在搜索…",
      websearch: "正在搜索…",
      webfetch: "正在读取网页…",
    };
    return labels[tools[0]] ?? "正在执行工具…";
  }
  return "等待响应…";
}
