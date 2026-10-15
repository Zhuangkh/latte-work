// @vitest-environment jsdom
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { TurnChanges } from "./protocol";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { TurnChangesCard } from "./TurnChangesCard";
import { turnTranscript } from "./TurnTranscript";
import { openTurnChanges, readWorkspace } from "./workspaceState";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
const changes: TurnChanges = {
  request_id: "r1",
  undo: "ready",
  interrupted: false,
  background_pending: false,
  summary: {
    entries: [{ path: "a.txt", status: "M", added: 2, removed: 1 }],
    added: 2,
    removed: 1,
    binary_files: 0,
    truncated: false,
    baseline_at: 1000,
    unavailable: null,
  },
};
const props = {
  changes,
  hostId: "local",
  sessionId: "s1",
  canUndo: true,
  open: vi.fn(),
};
it("shows per-turn counts and opens exact-file diffs without launching an agent", () => {
  render(<TurnChangesCard {...props} />);
  expect(screen.getByText("已编辑 1 个文件")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /a.txt/ }));
  expect(props.open).toHaveBeenCalledWith("a.txt");
  fireEvent.click(screen.getByRole("button", { name: "查看变更" }));
  expect(props.open).toHaveBeenLastCalledWith();
  expect(mocks.request).not.toHaveBeenCalled();
  openTurnChanges("turn-tabs", "s1", "r1", 1000, "a.txt");
  openTurnChanges("turn-tabs", "s1", "r2", 2000);
  openTurnChanges("turn-tabs", "s1", "r1", 1000);
  expect(readWorkspace("turn-tabs").tabs).toHaveLength(1);
  expect(readWorkspace("turn-tabs").current).toBe("diff");
});
it("sends undo once, records its result, and never retries a conflict", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  render(<TurnChangesCard {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "撤销" }));
  expect(
    (screen.getByRole("button", { name: "撤销中…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(mocks.request).toHaveBeenCalledExactlyOnceWith("local", {
    method: "undo_turn_changes",
    session_id: "s1",
    request_id: "r1",
  });
  finish({ kind: "turn_changes", changes: { ...changes, undo: "reverted" } });
  await screen.findByRole("button", { name: "已撤销" });
  cleanup();
  mocks.request.mockRejectedValue(new Error("文件在本轮结束后已改变，未撤销"));
  render(<TurnChangesCard {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "撤销" }));
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("文件在本轮结束后已改变，未撤销");
  await waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(2));
});
it("blocks undo for old, incomplete, background or unconfirmed turns", () => {
  for (const patch of [
    { canUndo: false },
    { changes: { ...changes, background_pending: true } },
    { changes: { ...changes, undo: "unknown" as const } },
    {
      changes: { ...changes, summary: { ...changes.summary, truncated: true } },
    },
  ]) {
    render(<TurnChangesCard {...props} {...patch} />);
    expect(
      (
        screen.getByRole("button", {
          name: /撤销|状态待核对/,
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    cleanup();
  }
});
it("keeps the changes card outside folded execution and replaces undo updates in place", () => {
  const turns = turnTranscript([
    {
      session_id: "s1",
      seq: 1,
      at: 1,
      event: { kind: "user", text: "edit", request_id: "r1" },
    },
    { session_id: "s1", seq: 2, at: 2, event: { kind: "text", text: "done" } },
    {
      session_id: "s1",
      seq: 3,
      at: 3,
      event: { kind: "turn_changes", changes },
    },
    {
      session_id: "s1",
      seq: 4,
      at: 4,
      event: { kind: "state", status: "completed", message: null },
    },
    {
      session_id: "s1",
      seq: 5,
      at: 5,
      event: {
        kind: "turn_changes",
        changes: { ...changes, undo: "reverted" },
      },
    },
  ]);
  const cards = turns[0].visible.filter(
    (item) => item.type === "event" && item.value.kind === "turn_changes",
  );
  expect(cards).toHaveLength(1);
  expect(cards[0]).toMatchObject({
    key: 3,
    value: { changes: { undo: "reverted" } },
  });
});

it.each([
  { truncated: false, unavailable: null },
  { truncated: true, unavailable: null },
  { truncated: false, unavailable: "无法读取 Git 状态，请确认项目是 Git 仓库" },
])(
  "hides empty snapshots regardless of collection limits or Git availability (%j)",
  (patch) => {
    render(
      <TurnChangesCard
        {...props}
        changes={{
          ...changes,
          background_pending: true,
          summary: {
            ...changes.summary,
            entries: [],
            added: 0,
            removed: 0,
            ...patch,
          },
        }}
      />,
    );
    expect(screen.queryByRole("region", { name: "本轮文件变更" })).toBeNull();
    expect(screen.queryByText(/已编辑|本轮变更未能记录/)).toBeNull();
  },
);
