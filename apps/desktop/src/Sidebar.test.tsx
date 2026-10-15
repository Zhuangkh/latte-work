// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { Sidebar } from "./Sidebar";
import { useState } from "react";
import { useSidebarPreview } from "./useSidebarPreview";
import { SidebarToggle } from "./SidebarToggle";
import type { Workbench } from "./useWorkbench";
import type { Session } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const session = (status: Session["status"], unread = false): Session => ({
  id: "s",
  project_id: "p",
  title: "Task",
  agent: "claude",
  native_id: null,
  model: null,
  effort: null,
  permission_mode: null,
  status,
  created_at: 1,
  custom_title: false,
  pinned_at: null,
  unread,
  archived: false,
});
const state = (sessions: Session[]) =>
  ({
    hosts: [{ id: "local", name: "Local" }],
    hostId: "local",
    connected: true,
    projects: [{ hostId: "local", id: "p", name: "Project", path: "/repo" }],
    projectId: "p",
    sessions,
    sessionId: "s",
    pinned: [],
    hostErrors: {},
    sidebarOpen: true,
    showArchived: false,
    openSession: vi.fn(),
  }) as unknown as Workbench;
const historySessions = (count: number, prefix: string, projectId = "p") =>
  Array.from({ length: count }, (_, i) => ({
    ...session("completed"),
    id: `${prefix}-${i + 1}`,
    title: `${prefix} ${i + 1}`,
    project_id: projectId,
  }));
beforeEach(() => {
  localStorage.clear();
  // Existing cases isolate project/pinned rows; recent navigation is covered below.
  localStorage.setItem(
    "latte-work.sidebar-sections.v1",
    JSON.stringify({ recent: false }),
  );
  vi.clearAllMocks();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
it("keeps the real sidebar and its portaled session menu usable during preview, then pins it", () => {
  vi.useFakeTimers();
  function PreviewSidebar() {
    const [open, setOpen] = useState(false);
    const [locked, setLocked] = useState(false);
    const preview = useSidebarPreview({
      open,
      enabled: true,
      interactionLocked: locked,
      toggle: () => setOpen((old) => !old),
    });
    return (
      <>
        {!open && <SidebarToggle toggle={() => {}} preview={preview} />}
        <Sidebar
          state={{ ...state([session("completed")]), sidebarOpen: open }}
          preview={preview}
          onInteractionChange={setLocked}
        />
      </>
    );
  }
  render(<PreviewSidebar />);
  const trigger = screen.getByRole("button", { name: "展开侧栏" });
  fireEvent.pointerEnter(trigger);
  act(() => vi.advanceTimersByTime(200));
  const panel = document.getElementById("project-sidebar")!;
  expect(panel.hidden).toBe(false);
  const more = screen.getByRole("button", { name: "Task 的更多操作" });
  act(() => more.focus());
  fireEvent.click(more);
  fireEvent.pointerLeave(trigger);
  fireEvent.pointerLeave(panel);
  act(() => vi.advanceTimersByTime(1000));
  expect(screen.getByRole("menu", { name: "Task 会话操作" })).toBeTruthy();
  expect(panel.hidden).toBe(false);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(more);
  expect(panel.hidden).toBe(false);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(panel.hidden).toBe(true);
  expect(document.activeElement).toBe(trigger);
  fireEvent.pointerEnter(trigger);
  act(() => vi.advanceTimersByTime(200));
  fireEvent.click(within(panel).getByRole("button", { name: "固定侧栏" }));
  expect(screen.queryByRole("button", { name: "展开侧栏" })).toBeNull();
  expect(screen.getByRole("button", { name: "收起侧栏" })).toBeTruthy();
  fireEvent.pointerLeave(panel);
  act(() => vi.advanceTimersByTime(1000));
  expect(panel.hidden).toBe(false);
});
it("shows five project conversations after filtering and lets the user reveal and collapse the rest", () => {
  const histories = historySessions(7, "History");
  const pinned = {
    ...session("completed"),
    id: "pinned",
    title: "Pinned history",
    pinned_at: 1,
    hostId: "local",
  };
  const cachedPin = {
    ...session("completed"),
    id: "cached-pin",
    title: "Cached pin",
    hostId: "local",
  };
  const props = {
    ...state([
      { ...session("completed"), title: "Archived history", archived: true },
      pinned,
      cachedPin,
      ...histories,
    ]),
    pinned: [pinned, { ...cachedPin, pinned_at: 1 }],
  };
  const view = render(<Sidebar state={props} />);
  const project = screen.getByRole("button", { name: "Project" });
  const list = within(document.getElementById("sessions-local:p")!);
  expect(list.getAllByRole("button", { name: /^History \d$/ })).toHaveLength(5);
  expect(list.queryByRole("button", { name: "Archived history" })).toBeNull();
  expect(list.queryByRole("button", { name: "Pinned history" })).toBeNull();
  expect(list.queryByRole("button", { name: "Cached pin" })).toBeNull();
  expect(list.queryByRole("button", { name: "History 6" })).toBeNull();
  const more = list.getByRole("button", { name: "展开显示 Project 的对话" });
  expect(more.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(more);
  expect(list.getAllByRole("button", { name: /^History \d$/ })).toHaveLength(7);
  expect(props.openSession).not.toHaveBeenCalled();
  fireEvent.click(list.getByRole("button", { name: "History 7" }));
  expect(props.openSession).toHaveBeenCalledWith({
    ...histories[6],
    hostId: "local",
  });
  fireEvent.click(project);
  fireEvent.click(project);
  expect(list.getAllByRole("button", { name: /^History \d$/ })).toHaveLength(7);
  fireEvent.click(
    list.getByRole("button", { name: "收起显示 Project 的对话" }),
  );
  expect(list.queryByRole("button", { name: "History 6" })).toBeNull();
  view.rerender(
    <Sidebar state={{ ...props, sessions: histories.slice(0, 5) }} />,
  );
  expect(
    list.queryByRole("button", { name: /显示 Project 的对话/ }),
  ).toBeNull();
});
it("keeps earlier projects expanded when selection moves or the recent section opens", async () => {
  const first = historySessions(2, "First");
  const second = historySessions(2, "Second", "other");
  const base = state(first);
  const props = {
    ...base,
    projects: [
      ...base.projects,
      { hostId: "local", id: "other", name: "Other", path: "/other" },
    ],
  };
  mocks.request.mockImplementation(async (_host, req) =>
    req.method === "recent_sessions"
      ? { kind: "recent_sessions", sessions: [], next: null }
      : {
          kind: "sessions",
          sessions: req.project_id === "other" ? second : first,
        },
  );
  const view = render(<Sidebar state={props} />);
  expect(
    screen
      .getByRole("button", { name: "Project" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  view.rerender(
    <Sidebar
      state={{
        ...props,
        projectId: "other",
        sessions: second,
        sessionId: second[0].id,
      }}
    />,
  );
  expect(
    screen
      .getByRole("button", { name: "Project" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  expect(
    screen.getByRole("button", { name: "Other" }).getAttribute("aria-expanded"),
  ).toBe("true");
  expect(screen.getByRole("button", { name: "First 1" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Second 1" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "最近" }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith(
      "local",
      expect.objectContaining({ method: "recent_sessions" }),
    ),
  );
  expect(
    screen
      .getByRole("button", { name: "Project" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  expect(
    screen.getByRole("button", { name: "Other" }).getAttribute("aria-expanded"),
  ).toBe("true");
  expect(props.openSession).not.toHaveBeenCalled();
});
it("keeps reveal and manual collapse choices separate for projects with the same ID on different hosts", async () => {
  const local = historySessions(7, "Local");
  const remote = historySessions(6, "Remote");
  const base = state(local);
  const props = {
    ...base,
    isHostConnected: () => true,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Remote host", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { hostId: "remote", id: "p", name: "Remote project", path: "/remote" },
    ],
  };
  mocks.request.mockImplementation(async (host) => ({
    kind: "sessions",
    sessions: host === "remote" ? remote : local,
  }));
  const view = render(<Sidebar state={props} />);
  fireEvent.click(
    screen.getByRole("button", { name: "展开显示 Project 的对话" }),
  );
  const remoteProject = screen.getByRole("button", { name: "Remote project" });
  fireEvent.click(remoteProject);
  await screen.findByRole("button", { name: "Remote 5" });
  expect(screen.getByRole("button", { name: "Local 7" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Remote 6" })).toBeNull();
  const switched = {
    ...props,
    hostId: "remote",
    projectId: "p",
    sessions: remote,
    sessionId: remote[0].id,
  };
  view.rerender(<Sidebar state={switched} />);
  expect(screen.getByRole("button", { name: "Local 7" })).toBeTruthy();
  expect(
    screen
      .getByRole("button", { name: "Project" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  fireEvent.click(
    screen.getByRole("button", { name: "展开显示 Remote project 的对话" }),
  );
  expect(screen.getByRole("button", { name: "Remote 6" })).toBeTruthy();
  fireEvent.click(
    screen.getByRole("button", { name: "收起显示 Project 的对话" }),
  );
  expect(screen.queryByRole("button", { name: "Local 6" })).toBeNull();
  expect(screen.getByRole("button", { name: "Remote 6" })).toBeTruthy();
  fireEvent.click(remoteProject);
  view.rerender(<Sidebar state={props} />);
  view.rerender(<Sidebar state={switched} />);
  expect(remoteProject.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("button", { name: "Remote 1" })).toBeNull();
  expect(props.openSession).not.toHaveBeenCalled();
});
it("distinguishes running, waiting and unread, and reflects folder expansion", () => {
  const { rerender } = render(
    <Sidebar state={state([session("running", true)])} />,
  );
  const project = screen.getByRole("button", { name: "Project" });
  expect(project.querySelector(".lucide-folder-open")).not.toBeNull();
  expect(screen.getByRole("img", { name: "执行中" })).toBeTruthy();
  expect(screen.queryByRole("img", { name: "未读" })).toBeNull();
  fireEvent.click(project);
  expect(project.querySelector(".lucide-folder")).not.toBeNull();
  expect(
    within(project).getByRole("img", { name: "项目中有对话执行中" }),
  ).toBeTruthy();
  fireEvent.click(project);
  rerender(<Sidebar state={state([session("waiting", true)])} />);
  expect(screen.getByRole("img", { name: "等待确认" })).toBeTruthy();
  rerender(<Sidebar state={state([session("completed", true)])} />);
  expect(screen.queryByRole("img", { name: "执行中" })).toBeNull();
  expect(screen.getByRole("img", { name: "未读" })).toBeTruthy();
  rerender(<Sidebar state={state([session("completed", false)])} />);
  expect(screen.queryByRole("img", { name: "未读" })).toBeNull();
});
it("opens recent history across projects and hosts without selecting on startup", async () => {
  localStorage.clear();
  const base = state([]);
  const remote = {
    ...session("completed"),
    id: "same",
    title: "Remote history",
    project_id: "remote-project",
  };
  mocks.request.mockImplementation(async (hostId, request) =>
    request.method === "recent_sessions"
      ? {
          kind: "recent_sessions",
          sessions: [
            {
              session:
                hostId === "local"
                  ? {
                      ...session("completed"),
                      id: "same",
                      title: "Local history",
                    }
                  : remote,
              updated_at: hostId === "local" ? 3 : 5,
            },
          ],
          next: null,
        }
      : { kind: "sessions", sessions: [] },
  );
  const props = {
    ...base,
    isHostConnected: () => true,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Remote", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      {
        hostId: "remote",
        id: "remote-project",
        name: "Remote project",
        path: "/remote",
      },
    ],
  };
  render(<Sidebar state={props} />);
  const recent = within(screen.getByRole("region", { name: "最近对话" }));
  await recent.findByRole("button", { name: "Remote history" });
  expect(
    recent
      .getAllByRole("button")
      .filter((button) => button.classList.contains("session-row"))
      .map((button) => button.textContent),
  ).toEqual(["Remote history", "Local history"]);
  expect(props.openSession).not.toHaveBeenCalled();
  fireEvent.click(recent.getByRole("button", { name: "Remote history" }));
  expect(props.openSession).toHaveBeenCalledWith(
    expect.objectContaining({
      id: "same",
      hostId: "remote",
      project_id: "remote-project",
    }),
  );
  expect(
    screen
      .getByRole("button", { name: "Remote project" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
  fireEvent.click(recent.getByRole("button", { name: "最近" }));
  expect(recent.queryByRole("button", { name: "Remote history" })).toBeNull();
  expect(
    JSON.parse(localStorage.getItem("latte-work.sidebar-sections.v1")!).recent,
  ).toBe(false);
});
it("keeps loaded recent history visible through a failed refresh and retry", async () => {
  localStorage.clear();
  let fail = false;
  mocks.request.mockImplementation(async () => {
    if (fail) throw new Error("Host is offline");
    return {
      kind: "recent_sessions",
      sessions: [{ session: session("completed"), updated_at: 1 }],
      next: { updated_at: 1, id: "s" },
    };
  });
  render(<Sidebar state={state([])} />);
  const recent = within(screen.getByRole("region", { name: "最近对话" }));
  await recent.findByRole("button", { name: "Task" });
  fail = true;
  fireEvent.click(recent.getByRole("button", { name: "加载更早的对话" }));
  await screen.findByRole("button", { name: "状态提示（1）" });
  expect(recent.queryByText(/加载失败/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  expect(screen.getByText("Local 的更早记录未能加载")).toBeTruthy();
  expect(recent.getByRole("button", { name: "Task" })).toBeTruthy();
  expect(recent.queryByText("暂无历史对话")).toBeNull();
  fail = false;
  mocks.request.mockResolvedValue({
    kind: "recent_sessions",
    sessions: [{ session: session("completed"), updated_at: 1 }],
    next: null,
  });
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  await waitFor(() => expect(screen.queryByText("Host is offline")).toBeNull());
});
it("offers a working history action while the sidebar is collapsed", async () => {
  localStorage.clear();
  const props = { ...state([]), toggleSidebar: vi.fn() };
  mocks.request.mockRejectedValue(new Error("History read failed"));
  const view = render(<Sidebar state={props} />);
  fireEvent.click(await screen.findByRole("button", { name: "状态提示（1）" }));
  view.rerender(<Sidebar state={{ ...props, sidebarOpen: false }} />);
  const action = screen.getByRole("button", {
    name: "查看最近",
  }) as HTMLButtonElement;
  expect(action.disabled).toBe(false);
  fireEvent.click(action);
  expect(props.toggleSidebar).toHaveBeenCalledOnce();
  mocks.request.mockResolvedValue({
    kind: "recent_sessions",
    sessions: [],
    next: null,
  });
  view.rerender(<Sidebar state={props} />);
  await waitFor(() =>
    expect(screen.queryByText("Error: History read failed")).toBeNull(),
  );
});
it("waits for remote readiness and reads history immediately after connection succeeds", async () => {
  localStorage.clear();
  const base = state([]);
  let ready = false;
  const props = {
    ...base,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Devbox", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { hostId: "remote", id: "rp", name: "Remote project", path: "/remote" },
    ],
    hostConnectionStatus: (id: string) =>
      id === "local" || ready
        ? ("connected" as const)
        : ("connecting" as const),
  };
  mocks.request.mockImplementation(async (host, req) => {
    if (req.method !== "recent_sessions")
      return { kind: "sessions", sessions: [] };
    if (host === "remote" && !ready) throw new Error("Host 未连接");
    return {
      kind: "recent_sessions",
      sessions:
        host === "remote"
          ? [
              {
                session: { ...session("completed"), project_id: "rp" },
                updated_at: 1,
              },
            ]
          : [],
      next: null,
    };
  });
  const view = render(<Sidebar state={props} />);
  await act(async () => {});
  expect(mocks.request.mock.calls.some(([host]) => host === "remote")).toBe(
    false,
  );
  const recent = within(screen.getByRole("region", { name: "最近对话" }));
  expect(recent.queryByText(/Devbox.*失败/)).toBeNull();
  expect(recent.queryByText("暂无历史对话")).toBeNull();
  ready = true;
  view.rerender(<Sidebar state={{ ...props }} />);
  await recent.findByRole("button", { name: "Task" });
  expect(recent.queryByText(/失败/)).toBeNull();
  expect(props.openSession).not.toHaveBeenCalled();
});
it("shows remote connection states independently of history results and selection", async () => {
  const base = state([]);
  let connection: "connecting" | "connected" | "failed" = "connecting";
  const props = {
    ...base,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Devbox", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { hostId: "remote", id: "rp", name: "Remote project", path: "/remote" },
      {
        hostId: "remote",
        id: "rp2",
        name: "Another remote project",
        path: "/remote2",
      },
    ],
    hostConnectionStatus: (id: string) =>
      id === "remote" ? connection : ("connected" as const),
  };
  mocks.request.mockResolvedValue({ kind: "sessions", sessions: [] });
  const view = render(<Sidebar state={props} />);
  expect(
    screen
      .getAllByRole("img", { name: "Devbox 正在连接" })
      .map((dot) => dot.dataset.state),
  ).toEqual(["connecting", "connecting"]);
  expect(
    within(screen.getByRole("button", { name: "Project" })).queryByRole("img", {
      name: /连接/,
    }),
  ).toBeNull();
  connection = "connected";
  view.rerender(<Sidebar state={{ ...props }} />);
  expect(
    screen
      .getAllByRole("img", { name: "Devbox 已连接" })
      .map((dot) => dot.dataset.state),
  ).toEqual(["connected", "connected"]);
  connection = "failed";
  view.rerender(
    <Sidebar state={{ ...props, hostErrors: { remote: "SSH 连接已关闭" } }} />,
  );
  expect(
    screen
      .getAllByRole("img", { name: "Devbox 连接失败" })
      .map((icon) => icon.querySelector("title")!.textContent),
  ).toEqual([
    "远程项目 · Devbox · 连接失败：SSH 连接已关闭",
    "远程项目 · Devbox · 连接失败：SSH 连接已关闭",
  ]);
  connection = "connected";
  view.rerender(<Sidebar state={{ ...props }} />);
  expect(screen.queryByRole("img", { name: "Devbox 连接失败" })).toBeNull();
  expect(screen.getAllByRole("img", { name: "Devbox 已连接" })).toHaveLength(2);
  await act(async () => {});
});
it("explains unavailable legacy history without an ineffective retry while keeping the host connected", async () => {
  localStorage.clear();
  const base = state([]);
  const props = {
    ...base,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Devbox", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { hostId: "remote", id: "rp", name: "Remote project", path: "/remote" },
    ],
    hostConnectionStatus: () => "connected" as const,
  };
  mocks.request.mockImplementation(async (host, req) => {
    if (req.method !== "recent_sessions")
      return { kind: "sessions", sessions: [] };
    if (host === "remote")
      throw new Error(
        "unknown variant `recent_sessions`, expected one of `hello`, `projects`, `sessions`",
      );
    return { kind: "recent_sessions", sessions: [], next: null };
  });
  const view = render(<Sidebar state={props} />);
  const recent = within(screen.getByRole("region", { name: "最近对话" }));
  await screen.findByRole("button", { name: "状态提示（1）" });
  expect(recent.queryByText(/完整历史暂不可用/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  const notice = screen.getByRole("dialog", { name: "状态提示" });
  expect(notice.textContent).toContain("Devbox 的完整历史暂不可用");

  expect(
    within(notice).getByText(/连接正常，仍可从项目列表打开对话/),
  ).toBeTruthy();
  expect(within(notice).getByText(/重新连接后会自动显示/)).toBeTruthy();
  expect(recent.queryByRole("button", { name: /重试/ })).toBeNull();
  expect(recent.queryByText(/Server 需更新/)).toBeNull();
  expect(recent.queryByText(/加载失败/)).toBeNull();
  expect(recent.queryByText("暂无历史对话")).toBeNull();
  expect(screen.getByRole("img", { name: "Devbox 已连接" }).dataset.state).toBe(
    "connected",
  );
  expect(props.openSession).not.toHaveBeenCalled();
  view.rerender(<Sidebar state={{ ...props, sidebarOpen: false }} />);
  expect(screen.getByText("Devbox 的完整历史暂不可用")).toBeTruthy();
});
it("refreshes collapsed background projects even while the current view rerenders", async () => {
  vi.useFakeTimers();
  const other = { hostId: "local", id: "other", name: "Other", path: "/other" };
  let status: Session["status"] = "running";
  mocks.request.mockImplementation(async () => ({
    kind: "sessions",
    sessions: [{ ...session(status, true), project_id: "other" }],
  }));
  const props = () => {
    const s = state([]);
    return { ...s, projects: [...s.projects, other] };
  };
  const view = render(<Sidebar state={props()} />);
  await act(async () => {});
  const project = screen.getByRole("button", { name: "Other" });
  expect(
    within(project).getByRole("img", { name: "项目中有对话执行中" }),
  ).toBeTruthy();
  status = "completed";
  for (let i = 0; i < 10; i++) {
    view.rerender(<Sidebar state={props()} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
  }
  expect(
    within(project).queryByRole("img", { name: "项目中有对话执行中" }),
  ).toBeNull();
  expect(
    within(project).getByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeTruthy();
  expect(mocks.request).toHaveBeenCalledTimes(2);
});

it("aggregates unread completions under collapsed projects with running and waiting priority", () => {
  const completed = session("completed", true);
  const running = { ...session("running"), id: "running" };
  const waiting = { ...session("waiting"), id: "waiting" };
  const view = render(<Sidebar state={state([completed])} />);
  const project = screen.getByRole("button", { name: "Project" });
  expect(
    within(project).queryByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeNull();
  fireEvent.click(project);
  expect(
    within(project).getByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeTruthy();
  view.rerender(<Sidebar state={state([completed, waiting, running])} />);
  expect(
    within(project).getByRole("img", { name: "项目中有对话执行中" }),
  ).toBeTruthy();
  expect(
    within(project).queryByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeNull();
  view.rerender(<Sidebar state={state([completed, waiting])} />);
  expect(
    within(project).getByRole("img", { name: "项目中有对话等待确认" }),
  ).toBeTruthy();
  view.rerender(<Sidebar state={state([completed])} />);
  expect(
    within(project).getByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeTruthy();
  view.rerender(<Sidebar state={state([{ ...completed, unread: false }])} />);
  expect(
    within(project).queryByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeNull();
  view.rerender(
    <Sidebar
      state={state([
        { ...completed, archived: true },
        { ...running, archived: true },
      ])}
    />,
  );
  expect(within(project).queryByRole("img", { name: /项目中有/ })).toBeNull();
});

it.each([
  ["running", "执行中"],
  ["waiting", "等待确认"],
  ["completed", "未读"],
] as const)(
  "keeps pinned %s activity only in the pinned row despite stale project metadata",
  (status, label) => {
    const pinned = { ...session(status, true), pinned_at: 1, hostId: "local" };
    const base = state([{ ...pinned, pinned_at: null }]);
    render(<Sidebar state={{ ...base, sessionId: "", pinned: [pinned] }} />);
    const project = screen.getByRole("button", { name: "Project" });
    fireEvent.click(project);
    expect(project.getAttribute("aria-expanded")).toBe("false");
    expect(within(project).queryByRole("img", { name: /项目中有/ })).toBeNull();
    expect(
      within(screen.getByRole("button", { name: "Task" })).getByRole("img", {
        name: label,
      }),
    ).toBeTruthy();
  },
);

it("updates collapsed project activity when conversations are pinned and unpinned", () => {
  const running = { ...session("running"), id: "running", title: "Running" };
  const waiting = { ...session("waiting"), id: "waiting", title: "Waiting" };
  const completed = session("completed", true);
  const base = { ...state([running, waiting, completed]), sessionId: "" };
  const view = render(<Sidebar state={base} />);
  const project = screen.getByRole("button", { name: "Project" });
  fireEvent.click(project);
  expect(
    within(project).getByRole("img", { name: "项目中有对话执行中" }),
  ).toBeTruthy();
  const pins = [{ ...running, pinned_at: 1, hostId: "local" }];
  view.rerender(<Sidebar state={{ ...base, pinned: pins }} />);
  expect(
    within(project).queryByRole("img", { name: "项目中有对话执行中" }),
  ).toBeNull();
  expect(
    within(project).getByRole("img", { name: "项目中有对话等待确认" }),
  ).toBeTruthy();
  pins.push({ ...waiting, pinned_at: 1, hostId: "local" });
  view.rerender(<Sidebar state={{ ...base, pinned: [...pins] }} />);
  expect(
    within(project).getByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeTruthy();
  view.rerender(
    <Sidebar
      state={{
        ...base,
        pinned: [...pins, { ...completed, pinned_at: 1, hostId: "local" }],
      }}
    />,
  );
  expect(within(project).queryByRole("img", { name: /项目中有/ })).toBeNull();
  view.rerender(<Sidebar state={{ ...base, pinned: [...pins] }} />);
  expect(
    within(project).getByRole("img", { name: "项目中有未读完成回复" }),
  ).toBeTruthy();
  view.rerender(<Sidebar state={base} />);
  expect(
    within(project).getByRole("img", { name: "项目中有对话执行中" }),
  ).toBeTruthy();
});

it("excludes pinned activity without suppressing same-id conversations on other hosts", async () => {
  const completed = {
    ...session("completed", true),
    pinned_at: 1,
    hostId: "local",
  };
  const base = state([]);
  const props = {
    ...base,
    sessionId: "",
    pinned: [completed],
    isHostConnected: () => true,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Remote", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { id: "p", hostId: "remote", name: "Remote project", path: "/remote" },
    ],
  };
  mocks.request.mockResolvedValue({
    kind: "sessions",
    sessions: [session("running")],
  });
  render(<Sidebar state={props} />);
  const project = screen.getByRole("button", { name: "Project" });
  fireEvent.click(project);
  expect(within(project).queryByRole("img", { name: /项目中有/ })).toBeNull();
  await waitFor(() =>
    expect(
      within(screen.getByRole("button", { name: "Remote project" })).getByRole(
        "img",
        { name: "项目中有对话执行中" },
      ),
    ).toBeTruthy(),
  );
});
it("opens row menus without selecting a conversation and keeps the trigger marked open", () => {
  const s = state([session("running")]);
  render(<Sidebar state={s} />);
  const more = screen.getByRole("button", { name: "Task 的更多操作" });
  fireEvent.click(more);
  expect(s.openSession).not.toHaveBeenCalled();
  expect(more.getAttribute("aria-expanded")).toBe("true");
  expect(more.closest(".session-item")?.getAttribute("data-menu-open")).toBe(
    "true",
  );
  expect(screen.getByRole("menu", { name: "Task 会话操作" })).toBeTruthy();
  fireEvent.keyDown(document, { key: "Escape" });
  expect(more.getAttribute("aria-expanded")).toBe("false");
  const project = screen.getByRole("button", { name: "Project 的更多操作" });
  fireEvent.click(project);
  expect(project.getAttribute("aria-expanded")).toBe("true");
});
it("disables close until the live conversation is open and skips it with the keyboard", async () => {
  const base = state([{ ...session("completed"), agent_session_open: false }]);
  base.sessionAction = vi.fn().mockResolvedValue(undefined);
  const view = render(<Sidebar state={base} />);
  fireEvent.click(screen.getByRole("button", { name: "Task 的更多操作" }));
  const closeButton = () =>
    screen.getByRole("menuitem", { name: "关闭" }) as HTMLButtonElement;
  expect(closeButton().disabled).toBe(true);
  fireEvent.click(closeButton());
  expect(base.sessionAction).not.toHaveBeenCalled();
  fireEvent.keyDown(document, { key: "End" });
  fireEvent.keyDown(document, { key: "ArrowUp" });
  expect(document.activeElement).toBe(
    screen.getByRole("menuitem", { name: "复制…" }),
  );
  const opened = {
    ...base,
    sessions: [{ ...base.sessions[0], agent_session_open: true }],
  };
  view.rerender(<Sidebar state={opened} />);
  expect(closeButton().disabled).toBe(false);
  view.rerender(<Sidebar state={{ ...opened, connected: false }} />);
  expect(closeButton().disabled).toBe(true);
  expect(closeButton().title).toBe("会话状态待确认");
  view.rerender(
    <Sidebar state={{ ...opened, agentSessionState: () => "restoring" }} />,
  );
  expect(closeButton().disabled).toBe(true);
  view.rerender(<Sidebar state={base} />);
  expect(closeButton().disabled).toBe(true);
  view.rerender(<Sidebar state={opened} />);
  await act(async () => fireEvent.click(closeButton()));
  expect(base.sessionAction).toHaveBeenCalledExactlyOnceWith("local", {
    method: "close_agent_session",
    session_id: "s",
  });
});
it("uses the target host's latest state for a pinned conversation menu", async () => {
  const local = { ...session("completed"), agent_session_open: true };
  const remote = { ...local, hostId: "remote", agent_session_open: false };
  const base = {
    ...state([local]),
    pinned: [remote],
    isHostConnected: () => true,
    sessionAction: vi.fn().mockResolvedValue(undefined),
  };
  const view = render(<Sidebar state={base} />);
  fireEvent.click(
    screen.getAllByRole("button", { name: "Task 的更多操作" })[0],
  );
  const closeButton = () =>
    screen.getByRole("menuitem", { name: "关闭" }) as HTMLButtonElement;
  expect(closeButton().disabled).toBe(true);
  view.rerender(
    <Sidebar
      state={{ ...base, pinned: [{ ...remote, agent_session_open: true }] }}
    />,
  );
  expect(closeButton().disabled).toBe(false);
  await act(async () => fireEvent.click(closeButton()));
  expect(base.sessionAction).toHaveBeenCalledExactlyOnceWith("remote", {
    method: "close_agent_session",
    session_id: "s",
  });
});
it("scopes quick pin/archive actions and prevents archiving active tasks", async () => {
  const s = state([session("running")]);
  s.sessionAction = vi.fn().mockResolvedValue(undefined);
  mocks.request.mockResolvedValue({
    kind: "sessions",
    sessions: [session("running")],
  });
  render(<Sidebar state={s} />);
  fireEvent.click(screen.getByRole("button", { name: "归档 Task" }));
  expect(s.sessionAction).not.toHaveBeenCalled();
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "置顶 Task" })),
  );
  expect(s.sessionAction).toHaveBeenCalledWith("local", {
    method: "pin_session",
    session_id: "s",
    pinned: true,
  });
});
it("keeps archived chats and archive navigation out of project sections", () => {
  render(
    <Sidebar state={state([{ ...session("completed"), archived: true }])} />,
  );
  expect(screen.queryByText("Task")).toBeNull();
  expect(screen.queryByText(/已归档/)).toBeNull();
});

it("mutes empty projects and restores their emphasis when a conversation exists", () => {
  const view = render(<Sidebar state={state([])} />);
  const project = screen.getByRole("button", { name: "Project" });
  expect(project.closest(".project-heading")?.classList.contains("empty")).toBe(
    true,
  );
  view.rerender(<Sidebar state={state([session("completed")])} />);
  expect(project.closest(".project-heading")?.classList.contains("empty")).toBe(
    false,
  );
  const pinned = { ...session("completed"), pinned_at: 1, hostId: "local" };
  view.rerender(<Sidebar state={{ ...state([]), pinned: [pinned] }} />);
  expect(project.closest(".project-heading")?.classList.contains("empty")).toBe(
    false,
  );
  expect(screen.queryByText("还没有任务")).toBeNull();
});

it("shows pinned sessions only once and restores them under the project after unpinning", () => {
  const pinned = { ...session("completed"), pinned_at: 1 };
  const base = state([pinned]);
  const view = render(
    <Sidebar state={{ ...base, pinned: [{ ...pinned, hostId: "local" }] }} />,
  );
  expect(screen.getAllByText("Task")).toHaveLength(1);
  expect(
    within(screen.getByRole("region", { name: "置顶会话" })).getByText("Task"),
  ).toBeTruthy();
  expect(screen.queryByText("还没有任务")).toBeNull();
  view.rerender(<Sidebar state={state([session("completed")])} />);
  expect(screen.getAllByText("Task")).toHaveLength(1);
  expect(screen.queryByRole("region", { name: "置顶会话" })).toBeNull();
});

it.each(["local", "remote"])(
  "opening a pinned conversation on %s preserves project expansion even when recent is hidden",
  (targetHost) => {
    const pinned = {
      ...session("completed"),
      id: "pin",
      title: "Pinned task",
      project_id: "other",
      pinned_at: 1,
      hostId: targetHost,
    };
    const base = state([session("completed")]);
    const props = {
      ...base,
      isHostConnected: () => true,
      hosts: [
        ...base.hosts,
        { id: "remote", name: "Remote host", ssh: "remote", server_path: null },
      ],
      projects: [
        ...base.projects,
        { hostId: targetHost, id: "other", name: "Other", path: "/other" },
      ],
      pinned: [pinned],
    };
    mocks.request.mockResolvedValue({ kind: "sessions", sessions: [pinned] });
    const view = render(<Sidebar state={props} />);
    const origin = screen.getByRole("button", { name: "Project" });
    const target = screen.getByRole("button", { name: "Other" });
    expect(origin.getAttribute("aria-expanded")).toBe("true");
    expect(target.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "Pinned task" }));
    expect(props.openSession).toHaveBeenCalledWith(pinned);
    view.rerender(
      <Sidebar
        state={{
          ...props,
          hostId: targetHost,
          projectId: "other",
          sessionId: pinned.id,
          sessions: [pinned],
        }}
      />,
    );
    expect(target.getAttribute("aria-expanded")).toBe("false");
    expect(origin.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(target);
    expect(target.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Pinned task" }));
    expect(target.getAttribute("aria-expanded")).toBe("true");
  },
);

it("shows delayed history feedback only beside the selected conversation", () => {
  vi.useFakeTimers();
  const s = state([
    session("completed"),
    { ...session("completed"), id: "other", title: "Other task" },
  ]);
  const view = render(<Sidebar state={s} historyPending />);
  expect(screen.queryByLabelText("正在加载对话")).toBeNull();
  act(() => vi.advanceTimersByTime(300));
  const indicator = screen.getByLabelText("正在加载对话");
  expect(indicator.closest("button")?.textContent).toContain("Task");
  expect(screen.queryByText("正在加载")).toBeNull();
  act(() => vi.advanceTimersByTime(4700));
  expect(screen.queryByText("正在加载")).toBeNull();
  expect(within(indicator).getAllByRole("status")).toHaveLength(1);
  view.rerender(<Sidebar state={s} historyPending={false} />);
  expect(screen.queryByLabelText("正在加载对话")).toBeNull();
});

it.each(["opening", "restoring"] as const)(
  "merges %s and history feedback into one indicator on the right",
  (phase) => {
    vi.useFakeTimers();
    const s = state([session("running")]);
    s.agentSessionState = () => phase;
    render(<Sidebar state={s} historyPending />);
    const row = screen.getByRole("button", { name: "Task" });
    const expectSingleIndicator = () => {
      const indicator = within(row).getByRole("status");
      expect(indicator.closest(".row-status")).not.toBeNull();
      expect(row.querySelector(".session-agent-open")).toBeNull();
      expect(within(row).queryByRole("img", { name: "执行中" })).toBeNull();
      expect(row.textContent).toBe("Task");
    };
    expectSingleIndicator();
    act(() => vi.advanceTimersByTime(5000));
    expectSingleIndicator();
    expect(within(row).getByLabelText("正在加载对话")).toBeTruthy();
  },
);

it("shows live Agent evidence in hover information without a leading icon or an open request", () => {
  vi.useFakeTimers();
  const opened = { ...session("completed"), agent_session_open: true };
  const base = state([opened]);
  const view = render(<Sidebar state={base} />);
  const hover = (row: HTMLElement) => {
    fireEvent.mouseEnter(row);
    act(() => vi.advanceTimersByTime(350));
    return screen.getByRole("tooltip");
  };
  const row = screen.getByRole("button", { name: "Task" });
  expect(row.firstElementChild?.className).toBe("session-label");
  expect(screen.queryByRole("tooltip")).toBeNull();
  const info = hover(row);
  expect(within(info).getByText("Task")).toBeTruthy();
  expect(within(info).getByText("Project")).toBeTruthy();
  expect(within(info).getByText("Local · 已完成")).toBeTruthy();
  expect(within(info).getByText("已打开")).toBeTruthy();
  expect(base.openSession).not.toHaveBeenCalled();
  expect(mocks.request).not.toHaveBeenCalled();
  const pinned = { ...opened, pinned_at: 1, hostId: "local" };
  view.rerender(
    <Sidebar state={{ ...base, sessions: [pinned], pinned: [pinned] }} />,
  );
  const pinnedRow = within(
    screen.getByRole("region", { name: "置顶会话" }),
  ).getByRole("button", { name: "Task" });
  expect(within(hover(pinnedRow)).getByText("已打开")).toBeTruthy();
  view.rerender(
    <Sidebar
      state={state([{ ...session("running"), agent_session_open: false }])}
    />,
  );
  expect(
    within(hover(screen.getByRole("button", { name: "Task" }))).queryByText(
      "已打开",
    ),
  ).toBeNull();
  expect(screen.getByRole("img", { name: "执行中" })).toBeTruthy();
  view.rerender(<Sidebar state={{ ...base, connected: false }} />);
  const disconnected = hover(screen.getByRole("button", { name: "Task" }));
  expect(within(disconnected).queryByText("已打开")).toBeNull();
  expect(within(disconnected).getByText("会话状态待确认")).toBeTruthy();
  view.rerender(<Sidebar state={state([session("completed")])} />);
  expect(within(screen.getByRole("tooltip")).queryByText("已打开")).toBeNull();
});

it("keeps live session evidence scoped to its host and hides it after host disconnect", () => {
  vi.useFakeTimers();
  const remote = {
    ...session("completed"),
    pinned_at: 1,
    agent_session_open: true,
    hostId: "remote",
  };
  const base = state([]);
  const props = {
    ...base,
    hosts: [
      ...base.hosts,
      { id: "remote", name: "Remote", ssh: "remote", server_path: null },
    ],
    projects: [
      ...base.projects,
      { hostId: "remote", id: "p", name: "Remote project", path: "/remote" },
    ],
    pinned: [remote],
    isHostConnected: (id: string) => id === "remote",
  };
  const view = render(<Sidebar state={props} />);
  fireEvent.mouseEnter(screen.getByRole("button", { name: "Task" }));
  act(() => vi.advanceTimersByTime(350));
  expect(within(screen.getByRole("tooltip")).getByText("已打开")).toBeTruthy();
  view.rerender(<Sidebar state={{ ...props, isHostConnected: () => false }} />);
  expect(within(screen.getByRole("tooltip")).queryByText("已打开")).toBeNull();
  expect(
    within(screen.getByRole("tooltip")).getByText("会话状态待确认"),
  ).toBeTruthy();
});

it("includes pinned and unselected conversations once in project hover counts and does not guess after disconnect", () => {
  vi.useFakeTimers();
  const opened = {
    ...session("completed"),
    agent_session_open: true,
    pinned_at: 1,
  };
  const other = {
    ...opened,
    id: "other",
    title: "Other pinned",
    hostId: "local",
  };
  const base = {
    ...state([
      opened,
      { ...session("completed"), id: "closed", agent_session_open: false },
      { ...opened, id: "archived", archived: true },
    ]),
    pinned: [{ ...opened, hostId: "local" }, other],
  };
  const view = render(<Sidebar state={base} />);
  fireEvent.mouseEnter(screen.getByRole("button", { name: "Project" }));
  act(() => vi.advanceTimersByTime(350));
  const info = screen.getByRole("tooltip");
  expect(within(info).getByText("Project")).toBeTruthy();
  expect(within(info).getByText("/repo")).toBeTruthy();
  expect(within(info).getByText("Local")).toBeTruthy();
  expect(within(info).getByText("3 个任务 · 2 个已打开")).toBeTruthy();
  expect(mocks.request).not.toHaveBeenCalled();
  view.rerender(<Sidebar state={{ ...base, connected: false }} />);
  expect(within(info).getByText("3 个任务 · 会话状态待确认")).toBeTruthy();
});
