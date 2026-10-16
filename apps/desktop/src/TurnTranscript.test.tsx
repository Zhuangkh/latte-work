// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import type { Event, EventKind, Status } from "./protocol";
import {
  TurnTranscript,
  turnTranscript,
  formatElapsed,
} from "./TurnTranscript";
import { MessageContent } from "./MessageContent";
import { UserMessage } from "./UserMessage";
afterEach(cleanup);
const user = (text: string): EventKind => ({
  kind: "user",
  text,
  request_id: text,
});
const text = (text: string): EventKind => ({ kind: "text", text });
const state = (status: Status): EventKind => ({
  kind: "state",
  status,
  message: null,
});
const tool: EventKind = { kind: "tool", id: "t", name: "Read", input: {} };
const result: EventKind = {
  kind: "tool_result",
  id: "t",
  content: "ok",
  is_error: false,
};
const events = (...values: EventKind[]): Event[] =>
  values.map((event, seq) => ({ seq, session_id: "s", at: seq, event }));
it("keeps sent queue bubbles visible after completion and deduplicates execution across groups", () => {
  const rows = events(
    user("original"),
    tool,
    { kind: "queued_message_sent", request_id: "queued", text: "follow up" },
    result,
    text("original final"),
    state("completed"),
    { kind: "user", request_id: "queued", text: "follow up" },
    text("follow up final"),
    state("completed"),
  );
  const view = render(
    <TurnTranscript events={rows}>
      {(item) =>
        item.type === "user" ? (
          <UserMessage text={item.text} at={item.at} />
        ) : item.type === "assistant" ? (
          <p>{item.text}</p>
        ) : (
          <p>tool</p>
        )
      }
    </TurnTranscript>,
  );
  const bubble = screen.getByText("follow up");
  expect(bubble.className).toBe("user-message");
  expect(bubble.closest("details")).toBeNull();
  expect(screen.getAllByText("follow up")).toHaveLength(1);
  expect(screen.getByText("follow up final")).toBeTruthy();
  expect(turnTranscript(rows)[0].process).toEqual([]);
  expect(turnTranscript(rows)[1].user).toMatchObject({ hidden: true });
  view.rerender(
    <TurnTranscript events={rows.slice(2)}>
      {(item) =>
        item.type === "user" ? (
          <UserMessage text={item.text} at={item.at} />
        ) : (
          <p>record</p>
        )
      }
    </TurnTranscript>,
  );
  expect(screen.getAllByText("follow up")).toHaveLength(1);
  expect(turnTranscript(rows.slice(2))[0].user).toBeUndefined();
});
it("folds only the process of each completed turn and keeps its final answer outside", () => {
  const rows = events(
    user("first"),
    text("progress"),
    tool,
    result,
    text("final"),
    state("completed"),
    user("second"),
    text("still working"),
    state("running"),
  );
  const turns = turnTranscript(rows);
  expect(turns).toHaveLength(2);
  expect(turns[0].process.map((i) => i.type)).toEqual(["assistant", "tool"]);
  expect(turns[0].visible).toEqual([
    expect.objectContaining({ text: "final" }),
  ]);
  expect(turns[1].process).toEqual([]);
  render(
    <TurnTranscript events={rows}>
      {(item) => (
        <p key={item.key}>
          {item.type === "assistant" || item.type === "user"
            ? item.text
            : "tool"}
        </p>
      )}
    </TurnTranscript>,
  );
  const details = screen.getByText("用时 0秒").closest("details")!;
  expect(details.open).toBe(false);
  expect(screen.queryByText("progress")).toBeNull();
  expect(details.contains(screen.getByText("final"))).toBe(false);
  details.open = true;
  fireEvent(details, new window.Event("toggle"));
  expect(details.open).toBe(true);
  expect(details.contains(screen.getByText("progress"))).toBe(true);
});
it.each(["running", "waiting", "failed", "unknown"] as Status[])(
  "does not collapse %s turns",
  (status) => {
    expect(
      turnTranscript(
        events(user("q"), text("progress"), tool, text("reply"), state(status)),
      )[0].process,
    ).toEqual([]);
  },
);
it("folds streamed progress when a turn completes", () => {
  const rows = events(user("q"), text("progress"), tool, result, text("final"));
  const content = (rows: Event[]) => (
    <TurnTranscript events={rows}>
      {(item) => (
        <p key={item.key}>{item.type === "assistant" ? item.text : "record"}</p>
      )}
    </TurnTranscript>
  );
  const view = render(content(rows));
  expect(screen.getByText("progress")).toBeTruthy();
  view.rerender(
    content([
      ...rows,
      ...events(state("completed")).map((e) => ({ ...e, seq: rows.length })),
    ]),
  );
  expect(screen.queryByText("progress")).toBeNull();
  expect(screen.getByText("final").closest("details")).toBeNull();
  expect(screen.getByText("用时 0秒").closest("details")!.open).toBe(false);
});
it.each(["completion", "session"])(
  "resets expansion for a new %s but preserves it for the same result",
  (change) => {
    const rows = events(
      user("q"),
      text("progress"),
      tool,
      result,
      text("final"),
      state("completed"),
    );
    const content = (rows: Event[]) => (
      <TurnTranscript events={rows}>
        {(item) => (
          <p key={item.key}>
            {item.type === "assistant" ? item.text : "record"}
          </p>
        )}
      </TurnTranscript>
    );
    const view = render(content(rows));
    const details = screen.getByText("用时 0秒").closest("details")!;
    details.open = true;
    fireEvent(details, new window.Event("toggle"));
    view.rerender(content([...rows]));
    expect(screen.getByText("progress")).toBeTruthy();
    expect(details.open).toBe(true);
    const next =
      change === "session"
        ? rows.map((e) => ({ ...e, session_id: "other" }))
        : [
            ...rows,
            {
              seq: rows.length,
              session_id: "s",
              at: rows.length,
              event: state("completed"),
            },
          ];
    view.rerender(content(next));
    expect(screen.getByText("用时 0秒").closest("details")!.open).toBe(false);
    expect(screen.queryByText("progress")).toBeNull();
    expect(screen.getByText("final").closest("details")).toBeNull();
  },
);
it("does not turn pre-tool commentary or incomplete history into a final answer", () => {
  expect(
    turnTranscript(
      events(user("q"), text("progress"), tool, result, state("completed")),
    )[0].process,
  ).toEqual([]);
  expect(
    turnTranscript(
      events(text("partial"), tool, text("reply"), state("completed")),
    )[0].process,
  ).toEqual([]);
  expect(
    turnTranscript(
      events(user("q"), text("only answer"), state("completed")),
    )[0].process,
  ).toEqual([]);
});
it("removes reply copying while retaining code copying", () => {
  render(<MessageContent text={"Answer\n\n```ts\nconst a = 1;\n```"} />);
  expect(screen.queryByRole("button", { name: "复制回复" })).toBeNull();
  expect(screen.getByRole("button", { name: "复制代码" })).toBeTruthy();
});

it("hides model metadata, preserves other notices and copies only the final answer", () => {
  const rows = events(
    user("q"),
    { kind: "notice", text: "模型：fable" },
    text("progress"),
    tool,
    result,
    { kind: "notice", text: "Provider：Local · 模型：fable" },
    { kind: "notice", text: "connection warning" },
    text("final"),
    state("completed"),
  );
  const turn = turnTranscript(rows)[0];
  expect(turn.process.filter((i) => i.type === "event")).toEqual([
    expect.objectContaining({
      value: { kind: "notice", text: "connection warning" },
    }),
  ]);
  render(
    <TurnTranscript events={rows}>
      {(item) => (
        <div key={item.key}>{item.type === "assistant" ? item.text : null}</div>
      )}
    </TurnTranscript>,
  );
  expect(screen.getAllByRole("button", { name: "复制回复" })).toHaveLength(1);
  expect(screen.getByRole("button", { name: "复制回复" }).textContent).toBe("");
  expect(formatElapsed(285000)).toBe("用时 4分 45秒");
  expect(formatElapsed(-1)).toBe("执行过程");
});
it("keeps copy for a completed direct answer without a process", () => {
  const rows = events(user("q"), text("final"), state("completed"));
  rows[2].at = Date.parse("2026-10-13T08:35:00Z");
  const view = render(
    <TurnTranscript events={rows}>
      {(item) => (
        <span key={item.key}>
          {item.type === "assistant" ? item.text : null}
        </span>
      )}
    </TurnTranscript>,
  );
  expect(screen.getByRole("button", { name: "复制回复" })).toBeTruthy();
  expect(view.container.querySelector("time")!.dateTime).toBe(
    "2026-10-13T08:35:00.000Z",
  );
  view.rerender(
    <TurnTranscript
      events={[
        ...rows,
        {
          seq: 3,
          session_id: "s",
          at: Date.parse("2026-10-14T08:35:00Z"),
          event: { kind: "notice", text: "later metadata" },
        },
      ]}
    >
      {(item) => (
        <span key={item.key}>
          {item.type === "assistant" ? item.text : null}
        </span>
      )}
    </TurnTranscript>,
  );
  expect(view.container.querySelector("time")!.dateTime).toBe(
    "2026-10-13T08:35:00.000Z",
  );
});
it("does not treat text before an approval-backed tool as a final answer", () => {
  const turns = turnTranscript(
    events(
      user("write"),
      text("progress"),
      tool,
      {
        kind: "approval",
        request_id: "a",
        tool_use_id: "t",
        tool: "Read",
        input: {},
      },
      { kind: "approval_resolved", request_id: "a", allow: true },
      result,
      state("completed"),
    ),
  );
  expect(turns[0].finalKey).toBeUndefined();
  expect(turns[0].process).toHaveLength(0);
});

it("summarizes confirmed stops, folds tools but preserves prose and rejected approvals", () => {
  const rows = events(
    user("q"),
    text("已有输出"),
    tool,
    result,
    {
      kind: "approval",
      request_id: "a",
      tool: "Write",
      input: {},
    },
    { kind: "approval_resolved", request_id: "a", allow: false },
    {
      kind: "state",
      status: "stopped",
      message: "已停止；已执行的文件改动不会撤销。",
    },
  );
  rows.at(-1)!.at = 13000;
  const turn = turnTranscript(rows)[0];
  expect(turn.elapsed).toBe("已停止 · 用时 13秒");
  expect(turn.finalKey).toBeUndefined();
  expect(turn.process.map((i) => i.type)).toEqual(["tool"]);
  expect(
    turn.visible.some((i) => i.type === "assistant" && i.text === "已有输出"),
  ).toBe(true);
  expect(
    turn.visible.some(
      (i) =>
        i.type === "event" &&
        i.value.kind === "approval" &&
        i.resolved &&
        i.decision === "denied",
    ),
  ).toBe(true);
  expect(
    turn.visible.some((i) => i.type === "event" && i.value.kind === "state"),
  ).toBe(false);
  render(
    <TurnTranscript events={rows}>
      {(item) => (
        <p key={item.key}>{item.type === "assistant" ? item.text : "记录"}</p>
      )}
    </TurnTranscript>,
  );
  expect(screen.getByText("已停止 · 用时 13秒").closest("details")!.open).toBe(
    false,
  );
  expect(screen.getByText("已有输出").closest("details")).toBeNull();
});
it("shows only a non-expandable stopped summary when stopped immediately", () => {
  render(
    <TurnTranscript events={events(user("q"), state("stopped"))}>
      {() => null}
    </TurnTranscript>,
  );
  const summary = screen.getByText("已停止");
  expect(summary.closest("details")).toBeNull();
  expect(summary.querySelector("svg")).toBeNull();
});
