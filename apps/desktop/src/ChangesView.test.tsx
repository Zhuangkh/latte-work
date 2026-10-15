// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ChangesView } from "./ChangesView";
import type { Request, TurnChanges } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const project = { id: "p", name: "Project", path: "/project" };
const props = { hostId: "local", project, active: true };
const info = {
  branch: "feature",
  head: "a".repeat(40),
  default_base: "refs/remotes/origin/main",
  refs: [
    { name: "origin/main", full_name: "refs/remotes/origin/main" },
    { name: "main", full_name: "refs/heads/main" },
  ],
  truncated: false,
};
const entries = [
  {
    path: "a.ts",
    previous_path: null,
    status: "M",
    added: 2,
    removed: 1,
    binary: false,
    untracked: false,
  },
  {
    path: "b.ts",
    previous_path: "old.ts",
    status: "R",
    added: 0,
    removed: 0,
    binary: false,
    untracked: false,
  },
];
const review = {
  base: "b".repeat(40),
  head: info.head,
  entries,
  added: 2,
  removed: 1,
  truncated: false,
};
beforeEach(() => {
  vi.clearAllMocks();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  mocks.request.mockImplementation(async (_host: string, req: Request) =>
    req.method === "git_info"
      ? { kind: "git_info", info }
      : req.method === "git_review"
        ? { kind: "git_review", review }
        : {
            kind: "content",
            text: `--- a/${"path" in req ? req.path : ""}\n+++ b/file\n@@ -10 +20,2 @@\n-old\n+new\n+extra\n`,
            truncated: false,
          },
  );
});
afterEach(cleanup);
async function scope(label: string) {
  fireEvent.click(screen.getByRole("combobox", { name: "变更范围" }));
  fireEvent.click(screen.getByRole("option", { name: label }));
  await waitFor(() =>
    expect(screen.getByRole("combobox", { name: "变更范围" }).textContent).toBe(
      label,
    ),
  );
}
it("defaults to the real branch base, filters the right list, routes selected diffs and switches layouts", async () => {
  render(<ChangesView {...props} />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "git_review",
    project_id: "p",
    scope: "branch",
    base: "refs/remotes/origin/main",
  });
  const list = screen.getByRole("complementary", { name: "变更文件" });
  expect(within(list).getAllByRole("button")).toHaveLength(2);
  fireEvent.change(screen.getByRole("textbox", { name: "筛选文件" }), {
    target: { value: "old" },
  });
  expect(within(list).getAllByRole("button")).toHaveLength(1);
  fireEvent.click(within(list).getByRole("button", { name: /b.ts/ }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenLastCalledWith(
      "local",
      expect.objectContaining({
        method: "git_review_diff",
        path: "b.ts",
        base: review.base,
        head: review.head,
      }),
    ),
  );
  await screen.findAllByRole("region", { name: "统一文件差异" });
  fireEvent.click(screen.getByRole("button", { name: "并排差异" }));
  expect(
    screen.getAllByRole("region", { name: "并排文件差异" })[0],
  ).toBeTruthy();
  expect(
    screen
      .getAllByRole("region", { name: "并排文件差异" })[0]
      .classList.contains("wrap"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "自动换行" }));
  expect(
    screen
      .getAllByRole("region", { name: "并排文件差异" })[0]
      .classList.contains("wrap"),
  ).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "更多差异选项" }));
  fireEvent.click(
    screen.getByRole("menuitemcheckbox", { name: "加载完整文件" }),
  );
  await waitFor(() =>
    expect(mocks.request).toHaveBeenLastCalledWith(
      "local",
      expect.objectContaining({ full_context: true }),
    ),
  );
  await screen.findAllByRole("region", { name: "并排文件差异" });
  expect(screen.queryByRole("menu")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "更多差异选项" }));
  expect(
    screen
      .getByRole("menuitemcheckbox", { name: "加载完整文件" })
      .getAttribute("aria-checked"),
  ).toBe("true");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "更多差异选项" }),
  );
  expect(screen.queryByRole("button", { name: "复制 diff" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "文件列表" }));
  expect(screen.queryByRole("complementary")).toBeNull();
});
it("changes comparison scopes and base without submitting agent commands", async () => {
  render(<ChangesView {...props} />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  fireEvent.click(screen.getByRole("combobox", { name: "比较基准" }));
  fireEvent.change(screen.getByRole("combobox", { name: "搜索分支" }), {
    target: { value: "main" },
  });
  fireEvent.click(screen.getByRole("option", { name: "main" }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith(
      "local",
      expect.objectContaining({
        method: "git_review",
        base: "refs/heads/main",
      }),
    ),
  );
  for (const [label, value] of [
    ["未提交", "worktree"],
    ["未暂存", "unstaged"],
    ["已暂存", "staged"],
  ]) {
    await scope(label);
    await screen.findAllByRole("region", { name: "统一文件差异" });
    expect(mocks.request).toHaveBeenCalledWith(
      "local",
      expect.objectContaining({
        method: "git_review",
        scope: value,
        base: null,
      }),
    );
  }
  fireEvent.click(screen.getByRole("button", { name: "刷新变更" }));
  await waitFor(() =>
    expect(
      mocks.request.mock.calls.filter(([, r]) => r.method === "git_info"),
    ).toHaveLength(2),
  );
  expect(
    mocks.request.mock.calls.every(([, r]) => r.method.startsWith("git_")),
  ).toBe(true);
});
it("fences stale host reads, performs no reads while hidden and reports unsupported servers", async () => {
  let resolve!: (value: unknown) => void;
  mocks.request.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const view = render(<ChangesView {...props} />);
  view.rerender(<ChangesView {...props} hostId="remote" />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  await act(async () =>
    resolve({ kind: "git_info", info: { ...info, branch: "stale" } }),
  );
  expect(screen.queryByText("stale")).toBeNull();
  const count = mocks.request.mock.calls.length;
  view.rerender(<ChangesView {...props} hostId="remote" active={false} />);
  expect(mocks.request).toHaveBeenCalledTimes(count);
  view.unmount();
  mocks.request.mockResolvedValue({ kind: "ok" });
  render(<ChangesView {...props} />);
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("不支持分支审阅");
});
it("distinguishes an empty branch from a filter with no matching files", async () => {
  render(<ChangesView {...props} />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  fireEvent.change(screen.getByRole("textbox", { name: "筛选文件" }), {
    target: { value: "missing" },
  });
  expect(screen.getByText("没有匹配的文件")).toBeTruthy();
  expect(screen.queryByText("此范围没有变更")).toBeNull();
  mocks.request.mockImplementation(async (_h, req) =>
    req.method === "git_info"
      ? { kind: "git_info", info }
      : {
          kind: "git_review",
          review: { ...review, entries: [], added: 0, removed: 0 },
        },
  );
  fireEvent.click(screen.getByRole("button", { name: "刷新变更" }));
  expect(await screen.findByText("此范围没有变更")).toBeTruthy();
});
const turn: TurnChanges = {
  request_id: "r1",
  undo: "reverted",
  interrupted: true,
  background_pending: true,
  summary: {
    entries: [{ path: "saved.txt", status: "M", added: 2, removed: 1 }],
    added: 2,
    removed: 1,
    binary_files: 0,
    truncated: false,
    baseline_at: 1,
    unavailable: null,
  },
};
it("reads the saved last turn outside Git, preserves its status and refreshes only the snapshot", async () => {
  mocks.request.mockImplementation(async (_host, req) => {
    if (req.method === "git_info") throw new Error("不是 Git 仓库");
    if (req.method === "last_turn_changes")
      return { kind: "last_turn_changes", changes: turn };
    if (req.method === "turn_change_diff")
      return {
        kind: "content",
        text: "@@ -1 +1 @@\n-old\n+saved\n",
        truncated: false,
      };
    throw new Error(`unexpected ${req.method}`);
  });
  const view = render(<ChangesView {...props} sessionId="s1" />);
  await screen.findByRole("button", { name: /^状态提示（/ });
  mocks.request.mockClear();
  await scope("上一轮");
  await screen.findByText("+saved");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText(/本轮修改已撤销/).textContent).toContain(
    "后台任务尚未结束",
  );
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "turn_change_diff",
    session_id: "s1",
    request_id: "r1",
    path: "saved.txt",
  });
  fireEvent.click(screen.getByRole("button", { name: "更多差异选项" }));
  expect(
    (
      screen.getByRole("menuitemcheckbox", {
        name: "加载完整文件",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(screen.getByText("回合快照仅保留采集时的上下文。")).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  fireEvent.click(screen.getByRole("button", { name: "刷新变更" }));
  await waitFor(() =>
    expect(
      mocks.request.mock.calls.filter(
        ([, req]) => req.method === "last_turn_changes",
      ),
    ).toHaveLength(2),
  );
  await screen.findByText("+saved");
  expect(
    mocks.request.mock.calls.every(([, req]) =>
      ["last_turn_changes", "turn_change_diff"].includes(req.method),
    ),
  ).toBe(true);
  const count = mocks.request.mock.calls.length;
  view.rerender(
    <ChangesView
      {...props}
      sessionId="s1"
      lastTurnVersion="r1"
      active={false}
    />,
  );
  expect(mocks.request).toHaveBeenCalledTimes(count);
});
it("does not invent a last turn when the conversation has no recorded snapshot", async () => {
  render(<ChangesView {...props} />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  mocks.request.mockClear();
  await scope("上一轮");
  expect(await screen.findByText("当前对话尚无已保存的回合快照")).toBeTruthy();
  expect(mocks.request).not.toHaveBeenCalled();
});
it("uses the server's empty snapshot result and reports unsupported responses", async () => {
  const previous = mocks.request.getMockImplementation()!;
  mocks.request.mockImplementation(async (host, req) =>
    req.method === "last_turn_changes"
      ? { kind: "last_turn_changes", changes: null }
      : previous(host, req),
  );
  render(<ChangesView {...props} sessionId="s1" />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  await scope("上一轮");
  await screen.findByText("当前对话尚无已保存的回合快照");
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "last_turn_changes",
    session_id: "s1",
  });
  mocks.request.mockResolvedValue({ kind: "ok" });
  fireEvent.click(screen.getByRole("button", { name: "刷新变更" }));
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("不支持回合快照");
});
it("reconciles an undo event for the same last turn without changing its saved diff", async () => {
  let saved = {
    ...turn,
    undo: "ready" as TurnChanges["undo"],
    interrupted: false,
    background_pending: false,
  };
  const previous = mocks.request.getMockImplementation()!;
  mocks.request.mockImplementation(async (host, req) =>
    req.method === "last_turn_changes"
      ? { kind: "last_turn_changes", changes: saved }
      : previous(host, req),
  );
  const view = render(
    <ChangesView {...props} sessionId="s1" lastTurnVersion="r1-ready" />,
  );
  await screen.findAllByRole("region", { name: "统一文件差异" });
  await scope("上一轮");
  await within(
    screen.getByRole("complementary", { name: "变更文件" }),
  ).findByRole("button", { name: /saved.txt/ });
  expect(screen.queryByText(/本轮修改已撤销/)).toBeNull();
  saved = { ...saved, undo: "reverted" };
  view.rerender(
    <ChangesView {...props} sessionId="s1" lastTurnVersion="r1-reverted" />,
  );
  await screen.findByText(/本轮修改已撤销/);
  expect(
    mocks.request.mock.calls.filter(
      ([, req]) => req.method === "last_turn_changes",
    ),
  ).toHaveLength(2);
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "turn_change_diff",
    session_id: "s1",
    request_id: "r1",
    path: "saved.txt",
  });
});
it.each([false, true])(
  "fences old session snapshots, including empty results (%s)",
  async (empty) => {
    let finish!: (value: unknown) => void;
    mocks.request.mockImplementation(async (_host, req) => {
      if (req.method === "git_info") return { kind: "git_info", info };
      if (req.method === "git_review") return { kind: "git_review", review };
      if (req.method === "last_turn_changes") {
        if (req.session_id === "s1")
          return new Promise((resolve) => {
            finish = resolve;
          });
        return {
          kind: "last_turn_changes",
          changes: { ...turn, request_id: "r2" },
        };
      }
      return {
        kind: "content",
        text: "@@ -1 +1 @@\n+new session\n",
        truncated: false,
      };
    });
    const view = render(
      <ChangesView {...props} sessionId="s1" lastTurnVersion="r1" />,
    );
    await screen.findAllByRole("region", { name: "统一文件差异" });
    await scope("上一轮");
    view.rerender(
      <ChangesView {...props} sessionId="s2" lastTurnVersion="r2" />,
    );
    await screen.findByText("+new session");
    await act(async () =>
      finish({
        kind: "last_turn_changes",
        changes: empty
          ? null
          : {
              ...turn,
              summary: {
                ...turn.summary,
                entries: [
                  { path: "stale.txt", status: "A", added: 1, removed: 0 },
                ],
              },
            },
      }),
    );
    expect(screen.queryByText("stale.txt")).toBeNull();
    expect(screen.getByText("+new session")).toBeTruthy();
    expect(
      mocks.request.mock.calls
        .filter(([, req]) => req.method === "turn_change_diff")
        .map(([, req]) => req),
    ).toEqual([
      {
        method: "turn_change_diff",
        session_id: "s2",
        request_id: "r2",
        path: "saved.txt",
      },
    ]);
  },
);
it("uses one page for a historical snapshot and Git scopes, preserving layout and returning to the exact request", async () => {
  const previous = mocks.request.getMockImplementation()!;
  mocks.request.mockImplementation(async (host, req) =>
    req.method === "turn_change_summary"
      ? {
          kind: "turn_changes",
          changes: { ...turn, request_id: req.request_id },
        }
      : previous(host, req),
  );
  render(<ChangesView {...props} sessionId="s1" requestId="historical" />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  expect(mocks.request).not.toHaveBeenCalledWith(
    "local",
    expect.objectContaining({ method: "git_info" }),
  );
  expect(screen.queryByRole("button", { name: "工作区" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "并排差异" }));
  await scope("未暂存");
  await screen.findAllByRole("region", { name: "并排文件差异" });
  await scope("本轮");
  await screen.findAllByRole("region", { name: "并排文件差异" });
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "turn_change_diff",
    session_id: "s1",
    request_id: "historical",
    path: "saved.txt",
  });
  expect(
    mocks.request.mock.calls.every(
      ([, req]) => !["send", "open_agent_session"].includes(req.method),
    ),
  ).toBe(true);
});
it("selects a real commit through the submenu and pins its diff IDs", async () => {
  const previous = mocks.request.getMockImplementation()!;
  const commit = { oid: "c".repeat(40), subject: "Fix review" };
  mocks.request.mockImplementation(async (host, req) =>
    req.method === "git_commits"
      ? {
          kind: "git_commits",
          head: commit.oid,
          commits: [commit],
          has_more: false,
        }
      : previous(host, req),
  );
  render(<ChangesView {...props} />);
  await screen.findAllByRole("region", { name: "统一文件差异" });
  fireEvent.click(screen.getByRole("combobox", { name: "变更范围" }));
  fireEvent.click(screen.getByRole("option", { name: "已提交" }));
  fireEvent.click(await screen.findByRole("option", { name: /Fix review/ }));
  await screen.findAllByRole("region", { name: "统一文件差异" });
  expect(
    screen.getByRole("combobox", { name: "变更范围" }).textContent,
  ).toContain("已提交 · cccccccc");
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "git_review",
    project_id: "p",
    scope: "commit",
    base: commit.oid,
  });
  expect(mocks.request).toHaveBeenCalledWith(
    "local",
    expect.objectContaining({
      method: "git_review_diff",
      scope: "commit",
      base: review.base,
      head: review.head,
    }),
  );
});
