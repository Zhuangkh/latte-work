// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { SubagentTranscript } from "./SubagentTranscript";
import type { Event, EventKind, Subagent } from "./protocol";
afterEach(cleanup);
const task: Subagent = {
  id: "a",
  native_id: "native-a",
  tool_use_id: "p",
  title: "检查项目",
  status: "running",
  summary: null,
  last_tool: "Read",
  started_at: 0,
  updated_at: 1000,
};
const rows = (...values: EventKind[]): Event[] =>
  values.map((event, seq) => ({ seq, session_id: "s", at: seq, event }));
const events = rows(
  { kind: "text", text: "child progress" },
  { kind: "tool", id: "read", name: "Read", input: { file_path: "child.txt" } },
  {
    kind: "tool_result",
    id: "read",
    content: "native read output",
    is_error: false,
  },
);
it("shows live child prose and native tool input/output, then folds it on completion", () => {
  const view = render(
    <SubagentTranscript task={task} events={events} elapsed="1s" />,
  );
  expect(screen.getByText("child progress")).toBeTruthy();
  expect(view.container.querySelector("time")).toBeNull();
  expect(screen.getByText("native read output")).toBeTruthy();
  expect(screen.getByText(/"file_path": "child.txt"/)).toBeTruthy();
  view.rerender(
    <SubagentTranscript
      task={{ ...task, status: "completed", summary: "child final" }}
      events={events}
      elapsed="3s"
    />,
  );
  expect(screen.queryByText("child progress")).toBeNull();
  expect(screen.getByText("child final").closest("details")).toBeNull();
  expect(view.container.querySelector("time")!.dateTime).toBe(
    new Date(task.updated_at).toISOString(),
  );
  expect(screen.getAllByRole("button", { name: "复制回复" })).toHaveLength(1);
  const details = screen.getByText("用时 3s").closest("details")!;
  expect(details.open).toBe(false);
  details.open = true;
  fireEvent(details, new window.Event("toggle"));
  expect(screen.getByText("child progress").closest("details")).toBe(details);
  view.rerender(
    <SubagentTranscript
      task={{ ...task, id: "b", status: "completed", summary: "other final" }}
      events={events}
      elapsed="3s"
    />,
  );
  expect(screen.getByText("用时 3s").closest("details")!.open).toBe(false);
});
it.each(["failed", "stopped", "unknown", "paused"] as const)(
  "retains actual process evidence for %s children",
  (status) => {
    render(
      <SubagentTranscript
        task={{ ...task, status }}
        events={events}
        elapsed="3s"
      />,
    );
    expect(screen.getByText("child progress")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "复制回复" })).toBeNull();
  },
);
it("does not claim a missing tool result succeeded after completion and marks partial snapshots", () => {
  render(
    <SubagentTranscript
      task={{ ...task, status: "completed" }}
      events={events.slice(0, 2)}
      elapsed="3s"
      incomplete
    />,
  );
  expect(screen.getByText("未收到最终输出")).toBeTruthy();
  const details = screen.getByText("用时 3s").closest("details")!;
  details.open = true;
  fireEvent(details, new window.Event("toggle"));
  expect(screen.getByText("结果待确认")).toBeTruthy();
  expect(screen.getByText(/部分原始记录暂不可读取/)).toBeTruthy();
});
it("shows the normalized delivered report instead of the task delivery receipt", () => {
  const receipt =
    "This agent's report was delivered to you as a message. Read it there.";
  render(
    <SubagentTranscript
      task={{ ...task, status: "completed", summary: receipt }}
      events={rows(
        {
          kind: "tool",
          id: "handback",
          name: "SubagentHandback",
          input: { message: "历史报告正文" },
        },
        {
          kind: "tool_result",
          id: "handback",
          content: [{ type: "text", text: '{"success":true}' }],
          is_error: false,
        },
        { kind: "text", text: "历史报告" },
        { kind: "text", text: "正文" },
      )}
      elapsed="3s"
      incomplete
    />,
  );
  expect(screen.getByText("历史报告正文").closest("details")).toBeNull();
  expect(screen.queryByText(receipt)).toBeNull();
  expect(screen.getByRole("button", { name: "复制回复" })).toBeTruthy();
  expect(screen.getByText("用时 3s").closest("details")!.open).toBe(false);
});

it("reads all pages before claiming a final answer or folding the process", () => {
  const view = render(
    <SubagentTranscript
      task={{ ...task, status: "completed", summary: "delivery receipt" }}
      events={events}
      elapsed="3s"
      readingMore
    />,
  );
  expect(screen.getByText("正在读取完整记录")).toBeTruthy();
  expect(screen.queryByText("delivery receipt")).toBeNull();
  expect(screen.queryByRole("button", { name: "复制回复" })).toBeNull();
  expect(screen.getByText("child progress").closest("details")).toBeNull();
  view.rerender(
    <SubagentTranscript
      task={{ ...task, status: "completed", summary: "delivery receipt" }}
      events={[
        ...events,
        {
          seq: 9,
          session_id: "s",
          at: 1,
          event: { kind: "text", text: "完整最终正文" },
        },
      ]}
      elapsed="3s"
    />,
  );
  expect(screen.getByText("完整最终正文").closest("details")).toBeNull();
  expect(screen.getByText("用时 3s").closest("details")!.open).toBe(false);
  expect(screen.queryByText(/显示上限/)).toBeNull();
});

it("keeps the original delegated prompt available above the folded process and final answer", () => {
  render(
    <SubagentTranscript
      task={{ ...task, status: "completed", summary: "Final report" }}
      events={events}
      elapsed="3s"
      prompt="Review the complete changes. Preserve this exact input."
    />,
  );
  const instruction = screen.getByText("任务指令").closest("details")!;
  expect(instruction.open).toBe(false);
  fireEvent.click(screen.getByText("任务指令"));
  expect(instruction.textContent).toContain(
    "Review the complete changes. Preserve this exact input.",
  );
  expect(screen.getByText("Final report").closest("details")).toBeNull();
});
