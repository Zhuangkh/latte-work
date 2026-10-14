// @vitest-environment jsdom
import type { ReactNode } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import App from "./App";
import type { Workbench } from "./useWorkbench";
import type { SidebarPreview } from "./useSidebarPreview";
const mocks = vi.hoisted(() => ({ state: {} as Workbench }));
vi.mock("./useWorkbench", () => ({ useWorkbench: () => mocks.state }));
vi.mock("./appLifecycle", () => ({ useAppLifecycle: () => {} }));
vi.mock("./useSubagents", () => ({
  useSubagents: () => ({ tasks: [], loading: false, error: "" }),
}));
vi.mock("./Sidebar", () => ({
  Sidebar: ({
    statusCenter,
    preview,
  }: {
    statusCenter: ReactNode;
    preview: SidebarPreview;
  }) => (
    <aside
      id="project-sidebar"
      hidden={!mocks.state.sidebarOpen && !preview.visible}
      ref={preview.panel}
      {...preview.panelProps}
    >
      <div className="window-drag">{statusCenter}</div>
      <button>侧栏项目</button>
    </aside>
  ),
}));
vi.mock("./Conversation", () => ({ Conversation: () => null }));
vi.mock("./TaskOverview", () => ({ TaskOverview: () => null }));
vi.mock("./Workspace", () => ({
  Workspace: ({
    statusCenter,
    conversationActions,
  }: {
    statusCenter: ReactNode;
    conversationActions: ReactNode;
  }) =>
    mocks.state.panel && mocks.state.workspace.expanded ? (
      <header data-testid="merged-header">
        {statusCenter}
        {conversationActions}
      </header>
    ) : null,
}));
vi.mock("./SettingsPage", () => ({ SettingsPage: () => null }));
vi.mock("./HostDialogs", () => ({ HostDialogs: () => null }));
vi.mock("./SshPasswordDialog", () => ({ SshPasswordDialog: () => null }));
beforeEach(() => {
  mocks.state = {
    hosts: [
      { id: "local", name: "本机" },
      { id: "remote", name: "Devbox" },
    ],
    hostId: "local",
    host: { id: "local", name: "本机" },
    connected: true,
    hostSupportsTerminal: () => true,
    hostSupportsUndo: () => true,
    connecting: false,
    hostErrors: { remote: "SSH 连接已关闭" },
    hostConnectionStatus: (id: string) =>
      id === "local" ? "connected" : "failed",
    events: [],
    projects: [],
    sessions: [],
    projectId: "",
    sessionId: "",
    workspaceId: "test",
    historyScope: "test",
    workspace: { expanded: false, conversationActive: true },
    error: "",
    setError: vi.fn(),
    refreshHost: vi.fn(),
    sidebarOpen: true,
    toggleSidebar: vi.fn(),
    panel: false,
    setPanel: vi.fn(),
    modal: null,
    setModal: vi.fn(),
    setSettingsTab: vi.fn(),
    leftWidth: 260,
    rightWidth: 480,
  } as unknown as Workbench;
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
it("keeps one status entry accessible with the sidebar collapsed and when the workspace is merged", () => {
  const view = render(<App />);
  expect(screen.getAllByRole("button", { name: "状态提示（1）" })).toHaveLength(
    1,
  );
  expect(
    screen.getByRole("button", { name: "状态提示（1）" }).closest("aside")?.id,
  ).toBe("project-sidebar");
  expect(
    document.querySelector(".topbar-actions .status-center-trigger"),
  ).toBeNull();
  expect(screen.queryByText("本机")).toBeNull();
  expect(screen.queryByText("SSH 连接已关闭")).toBeNull();
  expect(screen.getAllByRole("button", { name: "当前对话操作" })).toHaveLength(
    1,
  );
  expect(document.querySelectorAll(".topbar .titlebar-divider")).toHaveLength(
    1,
  );
  mocks.state = { ...mocks.state, sidebarOpen: false };
  view.rerender(<App />);
  expect(document.querySelectorAll(".topbar .titlebar-divider")).toHaveLength(
    2,
  );
  expect(screen.getAllByRole("button", { name: "状态提示（1）" })).toHaveLength(
    1,
  );
  expect(
    screen
      .getByRole("button", { name: "状态提示（1）" })
      .closest(".topbar-controls"),
  ).toBeTruthy();
  expect(
    within(document.querySelector(".topbar-controls")!).getAllByRole("button"),
  ).toEqual([
    screen.getByRole("button", { name: "状态提示（1）" }),
    screen.getByRole("button", { name: "展开侧栏" }),
    screen.getByRole("button", { name: "新对话" }),
  ]);
  mocks.state = {
    ...mocks.state,
    panel: true,
    workspace: { ...mocks.state.workspace, expanded: true },
  };
  view.rerender(<App />);
  expect(screen.getAllByRole("button", { name: "状态提示（1）" })).toHaveLength(
    1,
  );
  expect(
    within(screen.getByTestId("merged-header")).getByRole("button", {
      name: "状态提示（1）",
    }),
  ).toBeTruthy();
  expect(screen.getAllByRole("button", { name: "当前对话操作" })).toHaveLength(
    1,
  );
  expect(
    within(screen.getByTestId("merged-header")).getByRole("button", {
      name: "当前对话操作",
    }),
  ).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  expect(screen.getByText("Devbox 暂时无法连接")).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  mocks.state = { ...mocks.state, sidebarOpen: true };
  view.rerender(<App />);
  expect(screen.getAllByRole("button", { name: "状态提示（1）" })).toHaveLength(
    1,
  );
  expect(
    screen.getByRole("button", { name: "状态提示（1）" }).closest("aside")?.id,
  ).toBe("project-sidebar");
});
it("keeps a hovered sidebar visible while interacting with its status popover", () => {
  vi.useFakeTimers();
  mocks.state = { ...mocks.state, sidebarOpen: false };
  render(<App />);
  const trigger = screen.getByRole("button", { name: "展开侧栏" });
  fireEvent.pointerEnter(trigger, { pointerType: "mouse" });
  act(() => vi.advanceTimersByTime(200));
  const sidebar = document.getElementById("project-sidebar")!;
  expect(sidebar.hidden).toBe(false);
  expect(
    document.querySelector(".topbar-controls .titlebar-control-slot"),
  ).toBeTruthy();
  fireEvent.click(
    within(sidebar).getByRole("button", { name: "状态提示（1）" }),
  );
  const panel = screen.getByRole("dialog", { name: "状态提示" });
  fireEvent.pointerLeave(trigger);
  fireEvent.pointerLeave(sidebar);
  fireEvent.pointerDown(within(panel).getByText("Devbox 暂时无法连接"));
  act(() => vi.advanceTimersByTime(200));
  expect(sidebar.hidden).toBe(false);
  expect(screen.getByRole("dialog", { name: "状态提示" })).toBe(panel);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(
    within(sidebar).getByRole("button", { name: "状态提示（1）" }),
  );
  act(() => screen.getByRole("button", { name: "新对话" }).focus());
  act(() => vi.advanceTimersByTime(200));
  expect(sidebar.hidden).toBe(true);
  expect(screen.getAllByRole("button", { name: "状态提示（1）" })).toHaveLength(
    1,
  );
  expect(mocks.state.refreshHost).not.toHaveBeenCalled();
});
it("clears a background host failure after recovery and only reconnects on an explicit click", async () => {
  const view = render(<App />);
  expect(mocks.state.refreshHost).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "重新连接" })),
  );
  expect(mocks.state.refreshHost).toHaveBeenCalledWith(mocks.state.hosts[1]);
  mocks.state = {
    ...mocks.state,
    hostErrors: {},
    hostConnectionStatus: () => "connected",
  };
  view.rerender(<App />);
  expect(screen.getByRole("button", { name: "状态提示" })).toBeTruthy();
  expect(screen.queryByText("SSH 连接已关闭")).toBeNull();
  expect(screen.getByRole("dialog").textContent).toContain("当前没有状态提示");
});
