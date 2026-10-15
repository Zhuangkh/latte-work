import { expect, it } from "vitest";
import { executionStatus, subagentExecutionStatus } from "./executionStatus";
import { transcript } from "./transcript";
import type { Event, EventKind, Session, Subagent } from "./protocol";
const session = { id: "s", status: "running" } as Session;
const events = (...values: EventKind[]): Event[] =>
  values.map((event, i) => ({ seq: i, at: 0, session_id: "s", event }));
it("requires actual thinking evidence and keeps telemetry out of prose", () => {
  expect(executionStatus(session, [], true)).toBe("等待响应…");
  const thinking = { kind: "progress", phase: "thinking" } as const;
  expect(executionStatus(session, events(thinking), true)).toBe("正在思考…");
  expect(
    executionStatus(
      session,
      events(thinking, { kind: "progress", phase: "waiting" }),
      true,
    ),
  ).toBe("等待响应…");
  expect(
    executionStatus(
      session,
      events(thinking, { kind: "text", text: "hello" }),
      true,
    ),
  ).toBe("正在回复…");
  expect(
    transcript(
      events({ kind: "text", text: "a" }, thinking, {
        kind: "text",
        text: "b",
      }),
    ),
  ).toEqual([{ key: 0, type: "assistant", text: "ab" }]);
});
it("tracks parallel tools by ID and clears old turns and foreign sessions", () => {
  const read = { kind: "tool", id: "a", name: "Read", input: {} } as const;
  const command = { kind: "tool", id: "b", name: "Bash", input: {} } as const;
  expect(executionStatus(session, events(read), true)).toBe("正在读取文件…");
  expect(executionStatus(session, events(read, command), true)).toBe(
    "正在执行工具…",
  );
  const result = {
    kind: "tool_result",
    id: "a",
    content: "",
    is_error: false,
  } as const;
  expect(executionStatus(session, events(read, command, result), true)).toBe(
    "正在运行命令…",
  );
  expect(executionStatus(session, events(read, result), true)).toBe(
    "等待响应…",
  );
  expect(
    executionStatus(
      session,
      events(read, { kind: "user", text: "next", request_id: "r" }),
      true,
    ),
  ).toBe("等待响应…");
  expect(
    executionStatus(
      session,
      events(read).map((e) => ({ ...e, session_id: "other" })),
      true,
    ),
  ).toBe("等待响应…");
});
it("prioritizes confirmation, stopping, disconnect and terminal states", () => {
  const thinking = events({ kind: "progress", phase: "thinking" });
  expect(
    executionStatus({ ...session, status: "waiting" }, thinking, true),
  ).toBe("等待你的确认");
  expect(executionStatus(session, thinking, true, true)).toBe("正在停止…");
  expect(executionStatus(session, thinking, false)).toBe(
    "连接已断开，状态待确认",
  );
  for (const status of ["completed", "failed", "stopped", "unknown"] as const)
    expect(executionStatus({ ...session, status }, thinking, true)).toBeNull();
});

it("shares actual phase and tool status with subagent conversations while terminal and disconnected states remain explicit", () => {
  const child = { id: "child", status: "running" } as Subagent;
  expect(subagentExecutionStatus(child, "s", [], true)).toBe("等待响应…");
  expect(
    subagentExecutionStatus(
      child,
      "s",
      events({ kind: "progress", phase: "thinking" }),
      true,
    ),
  ).toBe("正在思考…");
  expect(
    subagentExecutionStatus(
      child,
      "s",
      events({ kind: "tool", id: "read", name: "Read", input: {} }),
      true,
    ),
  ).toBe("正在读取文件…");
  expect(
    subagentExecutionStatus(
      child,
      "s",
      events({ kind: "text", text: "hello" }),
      true,
    ),
  ).toBe("正在回复…");
  expect(
    subagentExecutionStatus({ ...child, status: "completed" }, "s", [], true),
  ).toBeNull();
  expect(
    subagentExecutionStatus({ ...child, status: "paused" }, "s", [], true),
  ).toBe("已暂停");
  expect(
    subagentExecutionStatus({ ...child, status: "unknown" }, "s", [], false),
  ).toBe("连接已断开，状态待确认");
});
