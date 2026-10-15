// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { TaskChangesView } from "./TaskChangesView";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
afterEach(cleanup);
HTMLElement.prototype.scrollIntoView = vi.fn();
const props = {
  hostId: "local",
  project: { id: "p", name: "P", path: "/p" },
  sessionId: "s",
  active: true,
};
it("opens task baseline diffs and labels legacy tasks without invented changes", async () => {
  mocks.request
    .mockResolvedValueOnce({
      kind: "change_summary",
      summary: {
        entries: [{ path: "a.txt", status: "M", added: 2, removed: 1 }],
        added: 2,
        removed: 1,
        binary_files: 0,
        unavailable: null,
        truncated: false,
      },
    })
    .mockResolvedValueOnce({
      kind: "content",
      text: "@@ -1 +1 @@\n-before\n+after",
      truncated: false,
    });
  const view = render(<TaskChangesView {...props} />);
  await screen.findAllByText("+2");
  await screen.findByText("−before");
  expect(mocks.request.mock.calls[1][1]).toEqual({
    method: "task_change_diff",
    session_id: "s",
    path: "a.txt",
  });
  view.unmount();
  mocks.request.mockResolvedValue({
    kind: "change_summary",
    summary: {
      entries: [],
      added: 0,
      removed: 0,
      unavailable: "此任务没有记录文件基线",
      truncated: false,
    },
  });
  render(<TaskChangesView {...props} />);
  await screen.findByText("此任务没有记录文件基线");
  expect(screen.queryByText("本任务没有文件变更")).toBeNull();
});
it("loads frozen turn diffs and fences responses after switching turns", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockReset().mockImplementation(async (_host, req) => {
    if (req.method === "turn_change_summary")
      return {
        kind: "turn_changes",
        changes: {
          request_id: req.request_id,
          undo: "ready",
          interrupted: false,
          background_pending: false,
          summary: {
            entries: [{ path: "a.txt", status: "M", added: 1, removed: 1 }],
            added: 1,
            removed: 1,
            truncated: false,
            unavailable: null,
          },
        },
      };
    if (req.request_id === "r1")
      return new Promise((resolve) => {
        finish = resolve;
      });
    return {
      kind: "content",
      text: "@@ -1 +1 @@\n-one\n+two",
      truncated: false,
    };
  });
  const view = render(
    <TaskChangesView {...props} requestId="r1" initialPath="a.txt" />,
  );
  await waitFor(() => expect(finish).toBeTypeOf("function"));
  view.rerender(
    <TaskChangesView {...props} requestId="r2" initialPath="a.txt" />,
  );
  await screen.findByText("+two");
  await act(async () =>
    finish({ kind: "content", text: "STALE_TURN", truncated: false }),
  );
  expect(screen.queryByText("STALE_TURN")).toBeNull();
  expect(mocks.request).toHaveBeenLastCalledWith("local", {
    method: "turn_change_diff",
    session_id: "s",
    request_id: "r2",
    path: "a.txt",
  });
  expect(screen.queryByRole("button", { name: "工作区" })).toBeNull();
});

it.each([false, true])(
  "does not show zero totals for an empty saved turn (partial=%s)",
  async (truncated) => {
    mocks.request.mockReset().mockResolvedValue({
      kind: "turn_changes",
      changes: {
        request_id: "r1",
        undo: "ready",
        interrupted: false,
        background_pending: false,
        summary: {
          entries: [],
          added: 0,
          removed: 0,
          binary_files: 0,
          baseline_at: 1,
          unavailable: null,
          truncated,
        },
      },
    });
    render(<TaskChangesView {...props} requestId="r1" />);
    await screen.findByText(
      truncated ? "采集范围内没有已记录的文件变更" : "本轮没有文件变更",
    );
    expect(screen.queryByText("+0")).toBeNull();
    expect(screen.queryByText("-0")).toBeNull();
    expect(screen.queryByText(/后台任务/)).toBeNull();
  },
);
