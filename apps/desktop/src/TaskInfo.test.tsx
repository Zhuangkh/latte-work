// @vitest-environment jsdom
import { StrictMode, Suspense } from "react";
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
import { TaskInfo } from "./TaskInfo";
import { readWorkspace } from "./workspaceState";
import { LinkHandling } from "./LinkHandling";
import {
  clearBrowserHistory,
  recordBrowserVisit,
  updateBrowserVisitTitle,
} from "./browserHistory";
import type { Request } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const props = {
  workspaceId: "",
  active: true,
  hostId: "local",
  connected: true,
  project: { id: "p", name: "Project", path: "/project" },
  sessionId: "s",
  events: [
    {
      seq: 1,
      at: 1,
      session_id: "s",
      event: {
        kind: "text" as const,
        text: "[Report](/project/report.md) [Docs](https://example.com/docs)",
      },
    },
  ],
  subagents: { tasks: [], loading: false, error: "", truncated: false },
};
beforeEach(() => {
  localStorage.clear();
  props.workspaceId = crypto.randomUUID();
  mocks.request
    .mockReset()
    .mockImplementation(async (_host: string, request: Request) =>
      request.method === "resolve_reference"
        ? {
            kind: "file_reference",
            entry: { path: "report.md", name: "report.md", directory: false },
          }
        : request.method === "git_review"
          ? {
              kind: "git_review",
              review: {
                entries: [{ path: "report.md" }],
                added: 8,
                removed: 2,
                truncated: false,
              },
            }
          : {
              kind: "sources",
              entries: [
                {
                  id: "Read",
                  name: "Read",
                  kind: "tool",
                  tools: ["Read"],
                  uses: 3,
                },
              ],
              truncated: false,
            },
    );
});

it("uses real page titles when available and keeps link rows aligned with files", async () => {
  render(
    <LinkHandling workspaceId={props.workspaceId}>
      <TaskInfo {...props} />
    </LinkHandling>,
  );
  const url = "https://example.com/docs";
  let link = screen.getByRole("link", { name: "Docs" });
  expect(link.classList.contains("task-info-file")).toBe(true);
  expect(
    link.querySelector('.task-info-site-icon[aria-hidden="true"]'),
  ).toBeTruthy();
  expect(within(link).getByText("example.com/docs").tagName).toBe("SMALL");
  act(() => recordBrowserVisit(url));
  expect(screen.getByRole("link", { name: "Docs" })).toBeTruthy();
  act(() => updateBrowserVisitTitle(url, "Queue reference — Example Docs"));
  link = await screen.findByRole("link", {
    name: "Queue reference — Example Docs",
  });
  expect(within(link).getByText("Queue reference — Example Docs").title).toBe(
    "Queue reference — Example Docs",
  );
  fireEvent.click(link);
  expect(readWorkspace(props.workspaceId).tabs).toContainEqual(
    expect.objectContaining({ kind: "browser", url }),
  );
  fireEvent.keyDown(link, { key: "F10", shiftKey: true });
  expect(screen.getByRole("menu", { name: "链接操作" })).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(document.activeElement).toBe(link);
  expect(
    mocks.request.mock.calls.every(([, request]) =>
      ["git_review", "sources"].includes(request.method),
    ),
  ).toBe(true);
});
afterEach(cleanup);
it("advances the incremental reader only after a render commits", () => {
  let suspended = true;
  const pending = new Promise<void>(() => {});
  const text = vi.fn(() => "[Committed report](/project/report.md)");
  const events = [
    {
      ...props.events[0],
      event: {
        kind: "text" as const,
        get text() {
          return text();
        },
      },
    },
  ];
  function Gate() {
    if (suspended) throw pending;
    return null;
  }
  const pane = () => (
    <StrictMode>
      <Suspense fallback={<p>Waiting to commit</p>}>
        <TaskInfo {...props} active={false} events={events} />
        <Gate />
      </Suspense>
    </StrictMode>
  );
  const view = render(pane());
  expect(screen.getByText("Waiting to commit")).toBeTruthy();
  expect(text).not.toHaveBeenCalled();
  suspended = false;
  view.rerender(pane());
  expect(screen.getByRole("button", { name: /Committed report/ })).toBeTruthy();
  expect(text).toHaveBeenCalledTimes(1);
  view.rerender(pane());
  expect(text).toHaveBeenCalledTimes(1);
});
it("matches visited page titles across anchors and resumes title updates after returning to the pane", async () => {
  const url = "https://example.com/docs";
  recordBrowserVisit(url, "Real document title");
  const events = [
    {
      ...props.events[0],
      event: {
        kind: "text" as const,
        text: `[Reference](${url}#details) https://unvisited.example/path`,
      },
    },
  ];
  const view = render(<TaskInfo {...props} events={events} />);
  await screen.findByRole("link", { name: "Real document title" });
  expect(
    screen.getByRole("link", { name: "unvisited.example/path" }),
  ).toBeTruthy();
  view.rerender(<TaskInfo {...props} events={events} active={false} />);
  act(() => updateBrowserVisitTitle(url, "Updated document title"));
  expect(
    screen.queryByRole("link", { name: "Updated document title" }),
  ).toBeNull();
  view.rerender(<TaskInfo {...props} events={events} />);
  await screen.findByRole("link", { name: "Updated document title" });
  act(() => clearBrowserHistory());
  expect(screen.getByRole("link", { name: "Reference" })).toBeTruthy();
});
it("renders ordered information and routes links and files into reusable tabs", async () => {
  render(
    <LinkHandling workspaceId={props.workspaceId}>
      <TaskInfo {...props} />
    </LinkHandling>,
  );
  await screen.findByText("1 个文件");
  expect(
    screen
      .getAllByRole("heading", { level: 2 })
      .map((node) =>
        within(node).getAllByRole("button")[0].getAttribute("aria-label"),
      ),
  ).toEqual([
    "变更概览",
    "子智能体",
    "待办事项",
    "调用的 Skill",
    "调用的 Tools",
    "任务产物",
    "任务资源",
  ]);
  expect(screen.getByText("+8")).toBeTruthy();
  expect(
    within(screen.getByRole("region", { name: "调用的 Tools" })).getByText(
      "3 次",
    ),
  ).toBeTruthy();
  fireEvent.click(screen.getByRole("link", { name: "Docs" }));
  expect(readWorkspace(props.workspaceId).tabs).toContainEqual(
    expect.objectContaining({
      kind: "browser",
      url: "https://example.com/docs",
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: /Report/ }));
  await waitFor(() =>
    expect(readWorkspace(props.workspaceId).tabs).toContainEqual(
      expect.objectContaining({
        kind: "files",
        file: "report.md",
        hostId: "local",
        project: props.project,
        sessionId: "s",
      }),
    ),
  );
  expect(
    mocks.request.mock.calls.every(([, value]) =>
      ["git_review", "sources", "resolve_reference"].includes(value.method),
    ),
  ).toBe(true);
});
it("offers older history without claiming missing records are empty", () => {
  const load = vi.fn();
  render(<TaskInfo {...props} hasEarlierHistory loadEarlierHistory={load} />);
  expect(screen.getAllByText("暂无记录").length).toBeGreaterThan(0);
  fireEvent.click(screen.getByRole("button", { name: "查看更早记录" }));
  expect(load).toHaveBeenCalledOnce();
});
it("does not read hidden/offline panes and fences late snapshots after session changes", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<TaskInfo {...props} active={false} />);
  await new Promise((done) => setTimeout(done, 120));
  expect(mocks.request).not.toHaveBeenCalled();
  view.rerender(<TaskInfo {...props} />);
  await waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(2));
  view.rerender(
    <TaskInfo {...props} hostId="remote" sessionId="other" connected={false} />,
  );
  finish({
    kind: "sources",
    entries: [{ id: "stale", name: "Stale", kind: "tool", uses: 7, tools: [] }],
    truncated: false,
  });
  await waitFor(() => expect(screen.queryByText("Stale")).toBeNull());
  expect(screen.queryByText("Report")).toBeNull();
  expect(screen.getByText("状态待确认")).toBeTruthy();
});
it("reports source failures through the shared status entry and supports explicit retry", async () => {
  mocks.request.mockRejectedValue(new Error("offline read"));
  render(<TaskInfo {...props} />);
  await waitFor(() =>
    expect(
      within(screen.getByRole("region", { name: "调用的 Tools" })).getByText(
        "暂不可用",
      ),
    ).toBeTruthy(),
  );
  fireEvent.click(screen.getByRole("button", { name: /状态提示/ }));
  expect(screen.getByText("任务工具列表暂不可用")).toBeTruthy();
  mocks.request.mockResolvedValue({
    kind: "sources",
    entries: [],
    truncated: false,
  });
  fireEvent.click(screen.getAllByRole("button", { name: "重试" })[1]);
  await waitFor(() =>
    expect(
      within(screen.getByRole("region", { name: "调用的 Tools" })).getByText(
        "暂无记录",
      ),
    ).toBeTruthy(),
  );
});
it("keeps scope information out of the header and uses one empty-state phrase", async () => {
  const view = render(
    <TaskInfo
      {...props}
      events={[]}
      hasEarlierHistory
      loadEarlierHistory={vi.fn()}
    />,
  );
  await screen.findByText("1 个文件");
  expect(screen.queryByText("Project")).toBeNull();
  expect(screen.queryByText(/按已加载的对话记录整理/)).toBeNull();
  for (const title of [
    "子智能体",
    "待办事项",
    "调用的 Skill",
    "任务产物",
    "任务资源",
  ]) {
    expect(
      within(screen.getByRole("region", { name: title })).getByText("暂无记录"),
    ).toBeTruthy();
  }
  expect(screen.getByRole("button", { name: "查看更早记录" }).title).toContain(
    "已加载",
  );
  view.rerender(<TaskInfo {...props} events={[]} />);
  expect(screen.queryByRole("button", { name: "查看更早记录" })).toBeNull();
  expect(
    within(screen.getByRole("region", { name: "待办事项" })).getByText(
      "暂无记录",
    ),
  ).toBeTruthy();
});

it("keeps Git reads independent of streamed events and refreshes tools at a bounded cadence", async () => {
  vi.useFakeTimers();
  try {
    const view = render(<TaskInfo {...props} running />);
    await act(async () => {});
    const count = (method: string) =>
      mocks.request.mock.calls.filter(([, r]) => r.method === method).length;
    expect(count("git_review")).toBe(1);
    const entries = [...props.events];
    for (let i = 0; i < 40; i++) {
      entries.push({
        seq: i + 2,
        at: i + 2,
        session_id: "s",
        event: { kind: "text", text: "fragment" },
      });
      view.rerender(<TaskInfo {...props} running events={[...entries]} />);
    }
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(count("git_review")).toBe(1);
    expect(count("sources")).toBe(2);
    view.rerender(<TaskInfo {...props} events={entries} />);
    await act(async () => {});
    expect(count("git_review")).toBe(2);
  } finally {
    cleanup();
    vi.useRealTimers();
  }
});

it("collapses groups independently, retains counts and leaves refresh separate", async () => {
  render(<TaskInfo {...props} hostId="collapse-refresh" />);
  await screen.findByText("1 个文件");
  const changes = screen.getByRole("button", { name: "变更概览" });
  const resources = screen.getByRole("button", { name: "任务资源" });
  fireEvent.click(changes);
  fireEvent.click(resources);
  expect(changes.getAttribute("aria-expanded")).toBe("false");
  expect(resources.getAttribute("aria-expanded")).toBe("false");
  expect(
    document.getElementById(resources.getAttribute("aria-describedby")!)
      ?.textContent,
  ).toBe("2");
  expect(screen.queryByRole("link", { name: "Docs" })).toBeNull();
  expect(
    screen
      .getByRole("button", { name: "调用的 Tools" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "刷新任务摘要" }));
  await waitFor(() =>
    expect(
      mocks.request.mock.calls.filter(
        ([, request]) => request.method === "git_review",
      ),
    ).toHaveLength(2),
  );
  expect(changes.getAttribute("aria-expanded")).toBe("false");
  expect(resources.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(resources);
  expect(screen.getByRole("link", { name: "Docs" })).toBeTruthy();
});
it("remembers this task's explicit choices across reopening without applying them to another task", () => {
  const view = render(<TaskInfo {...props} hostId="collapse-reopen" />);
  fireEvent.click(screen.getByRole("button", { name: "调用的 Skill" }));
  view.unmount();
  const reopened = render(<TaskInfo {...props} hostId="collapse-reopen" />);
  expect(
    screen
      .getByRole("button", { name: "调用的 Skill" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
  reopened.rerender(<TaskInfo {...props} hostId="another-task" />);
  expect(
    screen
      .getByRole("button", { name: "调用的 Skill" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  reopened.rerender(<TaskInfo {...props} hostId="collapse-reopen" />);
  expect(
    screen
      .getByRole("button", { name: "调用的 Skill" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
});
it("updates collapsed group counts without reopening them on incoming records", async () => {
  const view = render(<TaskInfo {...props} hostId="collapse-events" />);
  const header = screen.getByRole("button", { name: "任务资源" });
  fireEvent.click(header);
  view.rerender(
    <TaskInfo
      {...props}
      hostId="collapse-events"
      events={[
        ...props.events,
        {
          ...props.events[0],
          seq: 2,
          event: { kind: "text", text: "[New guide](https://example.com/new)" },
        },
      ]}
    />,
  );
  await waitFor(() =>
    expect(
      document.getElementById(header.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe("3"),
  );
  expect(header.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("link", { name: "New guide" })).toBeNull();
  fireEvent.click(header);
  expect(screen.getByRole("link", { name: "New guide" })).toBeTruthy();
});
