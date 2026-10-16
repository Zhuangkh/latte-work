import { describe, expect, it } from "vitest";
import { appendEvents, latestTurnChanges, transcript } from "./transcript";
import type { Event, EventKind, TurnChanges } from "./protocol";
const e = (seq: number, event: EventKind): Event => ({
  seq,
  event,
  at: 0,
  session_id: "s",
});
it("keeps the latest turn when an older turn's undo event arrives later", () => {
  const change = (id: string, baseline: number | null): TurnChanges => ({
    request_id: id,
    summary: {
      entries: [],
      added: 0,
      removed: 0,
      binary_files: 0,
      truncated: false,
      baseline_at: baseline,
      unavailable: null,
    },
    interrupted: false,
    background_pending: false,
    undo: "ready",
  });
  const first = change("r1", 1),
    second = change("r2", 2);
  expect(
    latestTurnChanges([
      e(1, { kind: "turn_changes", changes: first }),
      e(2, { kind: "turn_changes", changes: second }),
      e(3, { kind: "turn_changes", changes: { ...first, undo: "reverted" } }),
    ]),
  ).toEqual(second);
  expect(
    latestTurnChanges([
      e(1, { kind: "turn_changes", changes: change("r1", null) }),
      e(2, { kind: "turn_changes", changes: change("r2", null) }),
      e(3, {
        kind: "turn_changes",
        changes: { ...change("r1", null), undo: "unknown" },
      }),
    ])?.request_id,
  ).toBe("r2");
  expect(latestTurnChanges([])).toBeUndefined();
});
describe("durable transcript projection", () => {
  it("shows a handoff immediately and hides only its matching later execution bubble", () => {
    const receipt = {
      ...e(2, {
        kind: "queued_message_sent",
        request_id: "queued",
        text: "follow up",
      }),
      at: 2000,
    };
    const started = e(5, {
      kind: "user",
      request_id: "queued",
      text: "follow up",
    });
    const before = [
      e(1, { kind: "user", request_id: "head", text: "original" }),
      receipt,
    ];
    const after = appendEvents(before, [
      receipt,
      e(3, { kind: "text", text: "finishing original" }),
      started,
    ]);
    const visible = (rows: Event[]) =>
      transcript(rows).filter((item) => item.type === "user" && !item.hidden);
    expect(visible(before)).toHaveLength(2);
    expect(visible(after)).toEqual(visible(before));
    expect(visible(after)[1]).toMatchObject({
      key: 2,
      at: 2000,
      text: "follow up",
      handoff: true,
    });
    expect(visible([started])).toHaveLength(1);
    expect(
      visible([
        receipt,
        e(6, { kind: "user", request_id: "other", text: "follow up" }),
      ]),
    ).toHaveLength(2);
    expect(
      visible([receipt, { ...started, session_id: "other-session" }]),
    ).toHaveLength(2);
    expect(
      visible([
        receipt,
        e(6, { kind: "user", request_id: "queued", text: "different" }),
      ]),
    ).toHaveLength(2);
  });
  it("deduplicates replay while preserving token order", () => {
    const a = e(1, { kind: "text", text: "你" });
    const b = e(2, { kind: "text", text: "好" });
    expect(transcript(appendEvents([a], [a, b]))).toEqual([
      { key: 1, type: "assistant", text: "你好" },
    ]);
  });
  it("does not merge across user turns", () => {
    expect(
      transcript([
        e(1, { kind: "text", text: "a" }),
        e(2, { kind: "user", text: "b", request_id: "r" }),
        e(3, { kind: "text", text: "c" }),
      ]),
    ).toHaveLength(3);
  });
  it("expires approval controls after interruption or resolution", () => {
    const approval = e(1, {
      kind: "approval",
      request_id: "r",
      tool: "Bash",
      input: {},
    });
    expect(
      transcript([
        approval,
        e(2, { kind: "state", status: "unknown", message: "interrupted" }),
      ])[0],
    ).toMatchObject({ resolved: true });
    expect(
      transcript([
        approval,
        e(2, { kind: "approval_resolved", request_id: "r", allow: false }),
      ])[0],
    ).toMatchObject({ resolved: true });
  });
});
it("does not reactivate an interrupted approval when a later turn starts", () => {
  expect(
    transcript([
      e(1, { kind: "approval", request_id: "old", tool: "Write", input: {} }),
      e(2, { kind: "state", status: "stopped", message: null }),
      e(3, { kind: "user", text: "new", request_id: "new" }),
      e(4, { kind: "state", status: "running", message: null }),
    ])[0],
  ).toMatchObject({ resolved: true, decision: "expired" });
});

it("preserves each user message's original send time through replay", () => {
  const at = 1790760900000;
  const event = {
    ...e(1, { kind: "user", text: "继续", request_id: "r" }),
    at,
  };
  expect(transcript(appendEvents([event], [event]))).toEqual([
    { key: 1, type: "user", text: "继续", at },
  ]);
});
it("pairs saved question answers and marks unanswered terminal questions expired without manufacturing an answer", () => {
  const question: Event = {
    seq: 1,
    at: 0,
    session_id: "s",
    event: {
      kind: "question",
      request_id: "q",
      tool_use_id: null,
      questions: [],
    },
  };
  const answer: Event = {
    seq: 2,
    at: 1,
    session_id: "s",
    event: {
      kind: "question_resolved",
      request_id: "q",
      answers: [{ question_id: "q0", option_indices: [1], text: null }],
    },
  };
  const replied = transcript([question, answer]);
  expect(replied).toHaveLength(1);
  expect(replied[0]).toMatchObject({
    resolved: true,
    answers:
      answer.event.kind === "question_resolved" ? answer.event.answers : null,
  });
  const expired = transcript([
    question,
    {
      seq: 3,
      at: 2,
      session_id: "s",
      event: { kind: "state", status: "stopped", message: null },
    },
  ]);
  expect(expired[0]).toMatchObject({ resolved: true, answers: undefined });
});
