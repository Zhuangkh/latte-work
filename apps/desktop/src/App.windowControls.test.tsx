// @vitest-environment jsdom
import { useState } from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import App from "./App";
import type { Workbench } from "./useWorkbench";
import { updateWorkspace, useWorkspaceState } from "./workspaceState";

const mocks = vi.hoisted(() => ({
  platform: "windows",
  sidebarOpen: true,
  workspaceId: "",
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  isMaximized: vi.fn().mockResolvedValue(false),
  onResized: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("./platform", () => ({
  desktopPlatform: async () => mocks.platform,
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => mocks }));
vi.mock("./appLifecycle", () => ({ useAppLifecycle: () => {} }));
vi.mock("./useSubagents", () => ({
  useSubagents: () => ({ tasks: [], loading: false, error: "" }),
}));
vi.mock("./useWorkbench", () => ({
  useWorkbench: () => {
    const [workspace] = useWorkspaceState(mocks.workspaceId);
    const [modal, setModal] = useState<Workbench["modal"]>(null);
    const [panel, setPanel] = useState(true);
    return {
      hostId: "local",
      host: { id: "local", name: "本机" },
      hosts: [],
      hostErrors: {},
      hostSupportsTerminal: () => true,
      hostSupportsUndo: () => true,
      connected: false,
      connecting: false,
      events: [],
      projects: [],
      sessions: [],
      workspaceId: mocks.workspaceId,
      workspace,
      sidebarOpen: mocks.sidebarOpen,
      toggleSidebar: vi.fn(),
      leftWidth: 260,
      rightWidth: 480,
      error: "",
      modal,
      setModal,
      panel,
      setPanel,
      settingsTab: "browser",
      setSettingsTab: vi.fn(),
      setSettingsReturnToProject: vi.fn(),
      restoreWorkspace: () =>
        updateWorkspace(mocks.workspaceId, { expanded: false }),
    } as unknown as Workbench;
  },
}));
vi.mock("./Sidebar", () => ({
  Sidebar: ({ state }: { state: Workbench }) => (
    <button onClick={() => state.setModal("settings")}>打开设置</button>
  ),
}));
vi.mock("./Conversation", () => ({ Conversation: () => null }));
vi.mock("./TerminalPane", () => ({ TerminalPane: () => null }));
vi.mock("./TaskOverview", () => ({ TaskOverview: () => null }));
vi.mock("./HostDialogs", () => ({ HostDialogs: () => null }));
vi.mock("./ArchivedChats", () => ({ ArchivedChats: () => null }));
vi.mock("./AgentSettings", () => ({ AgentSettings: () => null }));
vi.mock("./ProviderSettings", () => ({ ProviderSettings: () => null }));
vi.mock("./SSHSettings", () => ({ SSHSettings: () => null }));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.platform = "windows";
  mocks.sidebarOpen = true;
  mocks.workspaceId = crypto.randomUUID();
  updateWorkspace(mocks.workspaceId, {
    tabs: [{ id: "subagents", kind: "subagents" }],
    current: "subagents",
    conversationActive: false,
  });
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);

function assertControls(headerSelector: string) {
  const header = document.querySelector<HTMLElement>(headerSelector)!;
  expect(header).toBeTruthy();
  for (const name of ["最小化", "最大化", "关闭"]) {
    const buttons = screen.getAllByRole("button", { name });
    expect(buttons).toHaveLength(1);
    const button = buttons[0];
    expect(within(header).getByRole("button", { name })).toBe(button);
    expect(
      button.closest('[hidden], [inert], [aria-hidden="true"]'),
    ).toBeNull();
    expect(button.hasAttribute("data-tauri-drag-region")).toBe(false);
    expect(button.querySelector("[data-tauri-drag-region]")).toBeNull();
    button.focus();
    expect(document.activeElement).toBe(button);
    fireEvent.click(button);
  }
}

it.each([true, false])(
  "keeps native actions in the active real header through expand, conversation selection, settings and restore (sidebar=%s)",
  async (sidebarOpen) => {
    mocks.sidebarOpen = sidebarOpen;
    render(<App />);
    await screen.findByRole("button", { name: "最小化" });
    assertControls(".topbar");
    fireEvent.click(screen.getByRole("button", { name: "展开工作区到主区域" }));
    await screen.findByRole("tab", { name: "新任务" });
    assertControls(".workspace.expanded .workspace-page > header");
    expect(document.querySelector(".topbar .window-controls")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "新任务" }));
    assertControls(".workspace.expanded .workspace-page > header");
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    assertControls(".settings-window-drag");
    fireEvent.click(screen.getByRole("button", { name: "返回工作台" }));
    assertControls(".workspace.expanded .workspace-page > header");
    fireEvent.click(screen.getByRole("button", { name: "分离右侧栏" }));
    await screen.findByRole("button", { name: "最小化" });
    assertControls(".topbar");
    expect(
      document.querySelector(".workspace-page .window-controls"),
    ).toBeNull();
    expect(mocks.minimize).toHaveBeenCalledTimes(6);
    expect(mocks.toggleMaximize).toHaveBeenCalledTimes(6);
    expect(mocks.close).toHaveBeenCalledTimes(6);
  },
);

it("preserves macOS native chrome in normal, merged and settings layouts", async () => {
  mocks.platform = "macos";
  render(<App />);
  await screen.findByRole("button", { name: "展开工作区到主区域" });
  expect(document.querySelector(".window-controls")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "展开工作区到主区域" }));
  await screen.findByRole("tab", { name: "新任务" });
  expect(document.querySelector(".window-controls")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
  expect(document.querySelector(".window-controls")).toBeNull();
  expect(mocks.onResized).not.toHaveBeenCalled();
});
