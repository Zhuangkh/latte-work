import { SourcePreview } from "./SourcePreview";
import { BrowserPane } from "./BrowserPane";
import { SiteIcon } from "./SiteIcon";
import { TaskChangesView } from "./TaskChangesView";
import { ChangesView } from "./ChangesView";
import {
  openSubagent,
  openArtifact,
  openBrowser,
  useWorkspaceState,
  type WorkspaceTab as Tab,
} from "./workspaceState";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Bot,
  Info,
  FileText,
  Globe2,
  Files,
  GitCompareArrows,
  TerminalSquare,
  Plus,
  X,
  PanelRight,
  Maximize2,
  MessageCircle,
} from "lucide-react";
import type { Project } from "./protocol";
import { request, message } from "./api";
import { WorkspaceFiles } from "./WorkspaceFiles";
import { SubagentsPanel } from "./SubagentsPanel";
import type { SubagentsState } from "./useSubagents";
import { TerminalPane } from "./TerminalPane";
import { SidebarToggle } from "./SidebarToggle";
import { useStatusIssues } from "./statusNotices";
import type { SidebarPreview } from "./useSidebarPreview";

function ConversationTab({
  title,
  active,
  select,
  onKeyDown,
}: {
  title: string;
  active: boolean;
  select: () => void;
  onKeyDown?: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
}) {
  return (
    <div className={`workspace-tab conversation-tab${active ? " active" : ""}`}>
      <button
        role="tab"
        id="conversation-workspace-tab"
        aria-selected={active}
        aria-controls="conversation-panel"
        tabIndex={active ? 0 : -1}
        title={title}
        onClick={select}
        onKeyDown={onKeyDown}
      >
        <MessageCircle size={16} />
        <span>{title}</span>
      </button>
    </div>
  );
}

type Kind = "terminal" | "files" | "diff" | "subagents" | "browser";

const labels = {
  taskInfo: "任务摘要",
  artifact: "文件产物",
  browser: "浏览器",
  terminal: "终端",
  files: "文件",
  diff: "变更",
  turnChanges: "本轮变更",
  subagents: "子智能体",
  subagent: "子智能体",
};
const icons = {
  taskInfo: Info,
  artifact: FileText,
  browser: Globe2,
  subagents: Bot,
  subagent: Bot,
  terminal: TerminalSquare,
  files: Files,
  diff: GitCompareArrows,
  turnChanges: GitCompareArrows,
};

export function Workspace({
  workspaceId,
  subagents,
  overview,
  taskInfo,
  conversationActions,
  statusCenter,
  sessionId,
  lastTurnVersion,
  conversationTitle = "对话",
  sidebarOpen = true,
  toggleSidebar,
  sidebarPreview,
  hostId,
  hostName,
  project,
  visible,
  connected,
  close,
}: {
  workspaceId: string;
  subagents?: SubagentsState;
  overview?: React.ReactNode;
  taskInfo?: React.ReactNode;
  conversationActions?: React.ReactNode;
  statusCenter?: React.ReactNode;
  sessionId?: string;
  lastTurnVersion?: string;
  conversationTitle?: string;
  sidebarOpen?: boolean;
  toggleSidebar?: () => void;
  sidebarPreview?: SidebarPreview;
  hostId: string;
  hostName: string;
  project?: Project;
  visible: boolean;
  connected: boolean;
  close: () => void;
}) {
  const [state, update] = useWorkspaceState(workspaceId);
  const expanded = state.expanded;
  const conversationActive = expanded && state.conversationActive;
  // Keep each visited conversation mounted so switching preserves its emulator.
  const [pages, setPages] = useState<
    {
      key: string;
      hostId: string;
      hostName: string;
      project?: Project;
      sessionId?: string;
    }[]
  >([]);
  useEffect(() => {
    const key = workspaceId;
    setPages((pages) =>
      pages.some((p) => p.key === key)
        ? pages.map((page) =>
            page.key === key
              ? { ...page, hostId, hostName, project, sessionId }
              : page,
          )
        : [...pages, { key, hostId, hostName, project, sessionId }],
    );
  }, [workspaceId, hostId, hostName, project]);
  return (
    <aside
      id="project-workspace"
      className={`workspace${expanded ? " expanded" : ""}${conversationActive ? " conversation-selected" : ""}`}
      hidden={!visible}
    >
      {pages.map((page) => {
        const selected = page.key === workspaceId;
        return (
          <WorkspacePage
            key={page.key}
            workspaceId={page.key}
            subagents={selected ? subagents : undefined}
            overview={selected ? overview : undefined}
            taskInfo={selected ? taskInfo : undefined}
            conversationActions={selected ? conversationActions : undefined}
            statusCenter={selected ? statusCenter : undefined}
            sessionId={selected ? sessionId : page.sessionId}
            lastTurnVersion={selected ? lastTurnVersion : undefined}
            conversationTitle={conversationTitle}
            sidebarOpen={sidebarOpen}
            toggleSidebar={toggleSidebar}
            sidebarPreview={sidebarPreview}
            hostId={selected ? hostId : page.hostId}
            hostName={selected ? hostName : page.hostName}
            project={selected ? project : page.project}
            selected={selected}
            active={selected && visible}
            connected={selected && connected && !!project}
            expanded={expanded}
            expand={() => update((state) => ({ expanded: !state.expanded }))}
            close={close}
          />
        );
      })}
    </aside>
  );
}

function WorkspacePage({
  workspaceId,
  subagents,
  overview,
  taskInfo,
  conversationActions,
  statusCenter,
  sessionId,
  lastTurnVersion,
  conversationTitle,
  sidebarOpen,
  toggleSidebar,
  sidebarPreview,
  hostId,
  hostName,
  project,
  selected,
  active,
  connected,
  expanded,
  expand,
  close,
}: {
  hostId: string;
  hostName: string;
  workspaceId: string;
  subagents?: SubagentsState;
  overview?: React.ReactNode;
  taskInfo?: React.ReactNode;
  conversationActions?: React.ReactNode;
  statusCenter?: React.ReactNode;
  sessionId?: string;
  lastTurnVersion?: string;
  conversationTitle: string;
  sidebarOpen: boolean;
  toggleSidebar?: () => void;
  sidebarPreview?: SidebarPreview;
  project?: Project;
  selected: boolean;
  active: boolean;
  connected: boolean;
  expanded: boolean;
  expand: () => void;
  close: () => void;
}) {
  const [state, update] = useWorkspaceState(workspaceId);
  const { tabs, current } = state;
  const setTabs = (value: Tab[] | ((tabs: Tab[]) => Tab[])) =>
    update((state) => ({
      tabs: typeof value === "function" ? value(state.tabs) : value,
    }));
  const conversationActive = expanded && state.conversationActive;
  const select = (id: string) =>
    update({ current: id, conversationActive: false });
  const selectConversation = () => update({ conversationActive: true });
  const navigateTabs = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const buttons = [
      ...(tablist.current?.querySelectorAll<HTMLButtonElement>(
        '[role="tab"]',
      ) ?? []),
    ];
    const index = buttons.indexOf(event.currentTarget);
    const next =
      buttons[
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? buttons.length - 1
            : (index + (event.key === "ArrowRight" ? 1 : -1) + buttons.length) %
              buttons.length
      ];
    next?.click();
    next?.focus();
  };
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useStatusIssues([
    {
      id: `workspace:${workspaceId}`,
      title: "工作区操作未完成",
      error: selected ? error : "",
      pending: busy,
      action: {
        label: "知道了",
        run: () => setError(""),
      },
    },
  ]);
  const [menu, setMenu] = useState(false);
  const [revision, refresh] = useState(0);
  const trigger = useRef<HTMLButtonElement>(null);
  const tabPrefix = useId();
  const tablist = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (active && current === "taskInfo" && !conversationActive)
      tablist.current
        ?.querySelector<HTMLButtonElement>('[data-task-info="true"]')
        ?.focus();
    if (active)
      tablist.current
        ?.querySelector<HTMLElement>('[aria-selected="true"]')
        ?.closest(".workspace-tab")
        ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [active, current, conversationActive]);
  useEffect(() => {
    if (!selected || !connected || !project) return;
    let disposed = false;
    const groups = new Map<
      string,
      { hostId: string; projectId: string; ids: Set<string> }
    >();
    for (const tab of tabs) {
      if (tab.kind !== "terminal") continue;
      const ownerHost = tab.hostId ?? hostId;
      const projectId = tab.terminal.project_id;
      const key = JSON.stringify([ownerHost, projectId]);
      const group = groups.get(key) ?? {
        hostId: ownerHost,
        projectId,
        ids: new Set<string>(),
      };
      group.ids.add(tab.id);
      groups.set(key, group);
    }
    if (!groups.size) return;
    void Promise.all(
      [...groups.values()].map(async (group) => {
        const response = await request(group.hostId, {
          method: "terminals",
          project_id: group.projectId,
        });
        if (response.kind !== "terminals")
          throw new Error("终端列表响应格式不匹配");
        return { group, terminals: response.terminals };
      }),
    )
      .then((results) => {
        if (disposed) return;
        // The conversation owns the tabs; each terminal retains its original host.
        setTabs((tabs) =>
          tabs.flatMap<Tab>((tab) => {
            if (tab.kind !== "terminal") return [tab];
            const result = results.find(
              (r) =>
                r.group.hostId === (tab.hostId ?? hostId) &&
                r.group.ids.has(tab.id),
            );
            if (!result) return [tab];
            const terminal = result.terminals.find(
              (t) => t.id === tab.id && t.project_id === result.group.projectId,
            );
            return terminal ? [{ ...tab, terminal }] : [];
          }),
        );
      })
      .catch((e) => {
        if (!disposed) setError(message(e));
      });
    return () => {
      disposed = true;
    };
  }, [selected, connected, hostId, project?.id, revision]);
  useEffect(() => {
    if (!active) setMenu(false);
  }, [active]);
  async function add(kind: Kind) {
    setMenu(false);
    setError("");
    if (kind === "browser") {
      try {
        openBrowser(workspaceId);
      } catch (e) {
        setError(message(e));
      }
      return;
    }
    if (!project) return;
    if (kind !== "terminal") {
      setTabs((tabs) =>
        tabs.some((t) => t.id === kind)
          ? tabs
          : [...tabs, { id: kind, kind, hostId, hostName, project }],
      );
      select(kind);
      return;
    }
    setBusy(true);
    const terminalId = crypto.randomUUID();
    update((state) => ({
      tabs: [
        ...state.tabs,
        {
          id: terminalId,
          kind: "terminal",
          hostId,
          hostName,
          project,
          terminal: {
            id: terminalId,
            project_id: project.id,
            title: "终端",
            exited: false,
            exit_code: null,
          },
        },
      ],
      current: terminalId,
      conversationActive: false,
    }));
    try {
      const response = await request(hostId, {
        method: "create_terminal",
        project_id: project.id,
        terminal_id: terminalId,
        cols: 80,
        rows: 24,
      });
      if (response.kind !== "terminal") throw new Error("无法创建终端");
      setTabs((tabs) =>
        tabs.map((tab) =>
          tab.id === terminalId
            ? { ...tab, kind: "terminal", terminal: response.terminal }
            : tab,
        ),
      );
    } catch (e) {
      setError(message(e));
      // Recover a creation whose reply was lost by listing, never blindly respawn.
      refresh((v) => v + 1);
    } finally {
      setBusy(false);
    }
  }
  async function remove(tab: Tab) {
    setBusy(true);
    setError("");
    try {
      if (tab.kind === "terminal")
        await request(tab.hostId ?? hostId, {
          method: "close_terminal",
          terminal_id: tab.id,
        });
      setTabs((tabs) => tabs.filter((t) => t.id !== tab.id));
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  const title = (tab: Tab) =>
    tab.kind === "artifact"
      ? tab.name
      : tab.kind === "terminal"
        ? `${tab.terminal.title} · ${tab.hostName ?? hostName}`
        : tab.kind === "subagent"
          ? (subagents?.tasks.find((task) => task.id === tab.taskId)?.title ??
            tab.title)
          : tab.kind === "turnChanges" || tab.kind === "browser"
            ? tab.title
            : labels[tab.kind];
  return (
    <div
      className={`workspace-page${tabs.length ? "" : " empty"}`}
      hidden={!selected}
    >
      <header data-tauri-drag-region="deep">
        {expanded && statusCenter}
        {expanded && !sidebarOpen && toggleSidebar && (
          <>
            <SidebarToggle toggle={toggleSidebar} preview={sidebarPreview} />
            <span className="titlebar-divider" aria-hidden="true" />
          </>
        )}
        <div
          ref={tablist}
          className="workspace-tabs"
          role="tablist"
          aria-label="工作区标签"
        >
          {expanded && selected && (
            <ConversationTab
              title={conversationTitle}
              active={conversationActive}
              select={selectConversation}
              onKeyDown={navigateTabs}
            />
          )}
          {tabs.map((tab) => {
            const Icon = icons[tab.kind];
            return (
              <div
                key={tab.id}
                className={`workspace-tab${!conversationActive && current === tab.id ? " active" : ""}`}
              >
                <button
                  data-task-info={tab.kind === "taskInfo" ? "true" : undefined}
                  role="tab"
                  id={`tab-${tabPrefix}-${tab.id}`}
                  aria-selected={!conversationActive && current === tab.id}
                  aria-controls={`pane-${tabPrefix}-${tab.id}`}
                  tabIndex={!conversationActive && current === tab.id ? 0 : -1}
                  title={title(tab)}
                  onClick={() => select(tab.id)}
                  onKeyDown={navigateTabs}
                >
                  {tab.kind === "browser" ? (
                    <SiteIcon url={tab.url} />
                  ) : (
                    <Icon size={16} />
                  )}
                  <span>{title(tab)}</span>
                </button>
                <button
                  className="tab-close icon-button"
                  aria-label={`关闭${title(tab)}`}
                  title={
                    tab.kind === "terminal"
                      ? "关闭终端并结束其中的进程"
                      : "关闭标签"
                  }
                  disabled={busy || (tab.kind === "terminal" && !connected)}
                  onClick={() => void remove(tab)}
                >
                  <X size={13} />
                </button>
              </div>
            );
          })}
        </div>
        {(expanded || tabs.length > 0) && (
          <button
            ref={trigger}
            className="icon-button workspace-add"
            aria-label="添加工作区标签"
            title="添加标签"
            aria-haspopup="menu"
            aria-expanded={menu}
            disabled={busy}
            onClick={() => setMenu((v) => !v)}
          >
            <Plus size={18} />
          </button>
        )}
        <div className="workspace-header-space" data-tauri-drag-region />
        {expanded && conversationActions}
        {expanded && overview}
        {expanded && <span className="titlebar-divider" aria-hidden="true" />}
        {!expanded && (
          <button
            className="icon-button"
            aria-label="展开工作区到主区域"
            title="展开"
            onClick={expand}
          >
            <Maximize2 size={15} />
          </button>
        )}
        <button
          className="icon-button"
          title={expanded ? "分离右侧栏" : "收起工作区"}
          aria-label={expanded ? "分离右侧栏" : "收起工作区"}
          onClick={close}
        >
          <PanelRight size={16} />
        </button>
      </header>
      {menu && trigger.current && (
        <AddMenu
          trigger={trigger.current}
          connected={connected && !!project}
          close={() => {
            setMenu(false);
            trigger.current?.focus();
          }}
          choose={(kind) => void add(kind)}
        />
      )}
      {tabs.map((tab) => (
        <section
          key={tab.id}
          id={`pane-${tabPrefix}-${tab.id}`}
          role="tabpanel"
          aria-labelledby={`tab-${tabPrefix}-${tab.id}`}
          className="workspace-pane"
          hidden={conversationActive || current !== tab.id}
        >
          {["files", "diff", "turnChanges"].includes(tab.kind) &&
          !(tab.project ?? project) ? (
            <div className="workspace-empty">选择项目后打开此标签。</div>
          ) : tab.kind === "taskInfo" ? (
            taskInfo
          ) : tab.kind === "artifact" ? (
            (tab.project ?? project) ? (
              <SourcePreview
                hostId={tab.hostId ?? hostId}
                projectId={(tab.project ?? project)!.id}
                sessionId={tab.sessionId ?? sessionId}
                source={{
                  path: tab.path,
                  name: tab.name,
                  directory: tab.directory,
                }}
                active={
                  active &&
                  !conversationActive &&
                  current === tab.id &&
                  connected
                }
                open={(file) =>
                  openArtifact(workspaceId, file, {
                    hostId: tab.hostId ?? hostId,
                    project: tab.project ?? project,
                    sessionId: tab.sessionId ?? sessionId,
                  })
                }
              />
            ) : (
              <div className="workspace-empty">选择项目后查看文件产物。</div>
            )
          ) : tab.kind === "browser" ? (
            <BrowserPane
              workspaceId={workspaceId}
              tab={tab}
              active={
                active && !conversationActive && current === tab.id && !menu
              }
            />
          ) : tab.kind === "terminal" ? (
            <TerminalPane
              hostId={tab.hostId ?? hostId}
              terminal={tab.terminal}
              active={
                active && !conversationActive && current === tab.id && !busy
              }
              connected={connected}
              onExited={() => void remove(tab)}
            />
          ) : tab.kind === "turnChanges" ? (
            <TaskChangesView
              hostId={hostId}
              project={tab.project ?? project!}
              sessionId={tab.sessionId}
              requestId={tab.requestId}
              initialPath={tab.file}
              active={
                active && !conversationActive && current === tab.id && connected
              }
            />
          ) : tab.kind === "diff" ? (
            <ChangesView
              key={tab.requestId ?? "git"}
              requestId={tab.requestId}
              snapshotAt={tab.snapshotAt}
              initialPath={tab.file}
              sessionId={tab.snapshotSessionId ?? sessionId}
              lastTurnVersion={lastTurnVersion}
              hostId={tab.hostId ?? hostId}
              project={tab.project ?? project!}
              active={
                active && !conversationActive && current === tab.id && connected
              }
            />
          ) : tab.kind === "subagents" || tab.kind === "subagent" ? (
            <SubagentsPanel
              taskId={tab.kind === "subagent" ? tab.taskId : undefined}
              hostId={hostId}
              sessionId={sessionId}
              connected={connected}
              active={
                active && !conversationActive && current === tab.id && connected
              }
              onOpen={(task) => openSubagent(workspaceId, task)}
              state={
                subagents ?? {
                  tasks: [],
                  loading: false,
                  error: "",
                  truncated: false,
                }
              }
            />
          ) : (
            <WorkspaceFiles
              hostId={tab.hostId ?? hostId}
              project={tab.project ?? project!}
              tab={tab.kind}
              reference={tab.reference}
              sessionId={tab.sessionId ?? sessionId}
              path={tab.path ?? ""}
              file={tab.file ?? ""}
              navigate={(value) =>
                setTabs((tabs) =>
                  tabs.map((t) => (t.id === tab.id ? { ...t, ...value } : t)),
                )
              }
              active={
                active && !conversationActive && current === tab.id && connected
              }
            />
          )}
        </section>
      ))}
      {!tabs.length && !conversationActive && (
        <div className="workspace-empty" aria-label="工作区入口">
          <div className="workspace-launchers">
            {(
              ["diff", "terminal", "files", "subagents", "browser"] as const
            ).map((kind) => {
              const Icon = icons[kind];
              return (
                <button
                  key={kind}
                  disabled={
                    busy || (kind !== "browser" && (!connected || !project))
                  }
                  onClick={() => void add(kind)}
                >
                  <Icon size={18} />
                  <span>{labels[kind]}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function AddMenu({
  trigger,
  connected,
  close,
  choose,
}: {
  trigger: HTMLButtonElement;
  connected: boolean;
  close: () => void;
  choose: (kind: Kind) => void;
}) {
  const menu = useRef<HTMLDivElement>(null);
  const rect = trigger.getBoundingClientRect();
  useEffect(() => {
    menu.current
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
    const outside = (e: PointerEvent) => {
      if (
        !menu.current?.contains(e.target as Node) &&
        !trigger.contains(e.target as Node)
      )
        close();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close();
      }
      if (e.key === "Tab") close();
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        const buttons = Array.from(
          menu.current?.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ) ?? [],
        );
        const index = buttons.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        buttons[
          e.key === "Home"
            ? 0
            : e.key === "End"
              ? buttons.length - 1
              : (index + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) %
                buttons.length
        ]?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", close);
    };
  }, [trigger, close]);
  return createPortal(
    <div
      className="workspace-add-menu project-context-menu"
      ref={menu}
      role="menu"
      aria-label="添加工作区标签"
      style={{
        left: Math.max(8, Math.min(rect.left, window.innerWidth - 232)),
        top: Math.min(rect.bottom + 6, window.innerHeight - 132),
      }}
    >
      {(["diff", "terminal", "files", "subagents", "browser"] as const).map(
        (kind) => {
          const Icon = icons[kind];
          return (
            <button
              key={kind}
              role="menuitem"
              disabled={kind !== "browser" && !connected}
              onClick={() => choose(kind)}
            >
              <Icon size={18} />
              <span>{labels[kind]}</span>
            </button>
          );
        },
      )}
    </div>,
    document.body,
  );
}
