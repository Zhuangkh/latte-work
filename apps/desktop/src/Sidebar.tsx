import { useEffect, useState } from "react";
import {
  Plus,
  Folder,
  FolderOpen,
  CirclePause,
  CircleAlert,
  CircleHelp,
  ChevronDown,
  ChevronRight,
  Settings2,
  Pin,
  Archive,
  PanelLeft,
  SquarePen,
  MoreHorizontal,
  PinOff,
  ArchiveRestore,
} from "lucide-react";
import { WorkingStatus } from "./WorkingStatus";
import { SessionHoverCard, ProjectHoverCard } from "./SidebarHoverCard";
import { agentSessionLabels, agentSessionState } from "./agentSessionState";
import { useHistoryLoadingIndicator } from "./useHistoryLoadingIndicator";
import { SessionActions } from "./SessionActions";
import { recentSessions, type HostedSession } from "./sessionNavigation";
import { useRecentSessions } from "./useRecentSessions";
import { useStatusIssues, type StatusIssue } from "./statusNotices";
import { ProjectActions } from "./ProjectActions";
import { RemoteProjectIcon } from "./RemoteProjectIcon";
import { RemoteGlobeIcon } from "./RemoteGlobeIcon";
import type { HostedProject } from "./projectCatalog";
import { message, request } from "./api";
import { statusNames } from "./Conversation";
import type { Session } from "./protocol";
import type { Workbench } from "./useWorkbench";
import type { SidebarPreview } from "./useSidebarPreview";
const PROJECT_SESSION_LIMIT = 5;
export function Sidebar({
  state,
  historyPending = state.historyLoading,
  preview,
  onInteractionChange,
  statusCenter,
}: {
  state: Workbench;
  historyPending?: boolean;
  preview?: SidebarPreview;
  onInteractionChange?: (active: boolean) => void;
  statusCenter?: React.ReactNode;
}) {
  const visible = state.sidebarOpen || !!preview?.visible;
  const loadingScope = JSON.stringify([
    state.hostId,
    state.sessionId,
    state.viewRevision,
  ]);
  const loading = useHistoryLoadingIndicator(!!historyPending, loadingScope);
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [sessionContext, setSessionContext] = useState<{
    session: HostedSession;
    point: { x: number; y: number };
  } | null>(null);
  const [context, setContext] = useState<{
    project: HostedProject;
    point: { x: number; y: number };
  } | null>(null);
  const interactionActive = !!sessionContext || !!context;
  useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);
  useEffect(() => {
    if (!visible) {
      setSessionContext(null);
      setContext(null);
    }
  }, [visible]);
  const [expandedProjects, setExpandedProjects] = useState<
    Record<string, boolean>
  >({});
  const [showAllProjects, setShowAllProjects] = useState<
    Record<string, boolean>
  >({});
  const [sessionCache, setSessionCache] = useState<Record<string, Session[]>>(
    {},
  );
  const [loadingProjects, setLoadingProjects] = useState<
    Record<string, boolean>
  >({});
  const [projectErrors, setProjectErrors] = useState<Record<string, string>>(
    {},
  );
  const [sections, setSections] = useState(() => {
    const defaults = { pinned: true, projects: true, recent: true };
    try {
      const value: unknown = JSON.parse(
        localStorage.getItem("latte-work.sidebar-sections.v1") ?? "null",
      );
      if (!value || typeof value !== "object") return defaults;
      return {
        pinned:
          "pinned" in value && typeof value.pinned === "boolean"
            ? value.pinned
            : true,
        projects:
          "projects" in value && typeof value.projects === "boolean"
            ? value.projects
            : true,
        recent:
          "recent" in value && typeof value.recent === "boolean"
            ? value.recent
            : true,
      };
    } catch {
      return defaults;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(
        "latte-work.sidebar-sections.v1",
        JSON.stringify(sections),
      );
    } catch {
      /* Nonessential presentation preference. */
    }
  }, [sections]);
  const sectionToggle = (key: keyof typeof sections, label: string) => (
    <button
      className="section-toggle"
      aria-expanded={sections[key]}
      aria-controls={`sidebar-${key}`}
      onClick={() => setSections((old) => ({ ...old, [key]: !old[key] }))}
    >
      {label}
      {sections[key] ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
    </button>
  );
  const {
    hosts,
    hostId,
    connected,
    createSession,
    setError,
    projects,
    projectId,
    openProject,
    hostErrors,
    sessions,
    sessionId,
    setModal,
  } = state;
  const connectionStatus = (id: string) =>
    state.hostConnectionStatus?.(id) ??
    (hostErrors[id]
      ? "failed"
      : (state.isHostConnected?.(id) ?? (id === hostId && connected))
        ? "connected"
        : "connecting");
  const selectedIsPinned = state.pinned.some(
    (s) =>
      s.hostId === hostId && s.project_id === projectId && s.id === sessionId,
  );
  useEffect(() => {
    if (!projectId || sections.recent || selectedIsPinned) return;
    const key = `${hostId}:${projectId}`;
    setExpandedProjects((old) =>
      old[key] === undefined ? { ...old, [key]: true } : old,
    );
  }, [hostId, projectId, sections.recent, selectedIsPinned]);
  const recent = useRecentSessions(
    hosts
      .filter((host) => connectionStatus(host.id) === "connected")
      .map((host) => host.id),
    sections.recent && visible,
    state.hostConnectionVersions,
  );
  const recentRows = recentSessions(
    Object.fromEntries(
      hosts.map((host) => [host.id, recent.hosts[host.id]?.sessions ?? []]),
    ),
    projects,
    hostId,
    sessions,
    state.pinned,
  );
  useEffect(() => {
    if (!projectId || !connected) return;
    const key = `${hostId}:${projectId}`;
    setSessionCache((old) => ({ ...old, [key]: sessions }));
  }, [hostId, projectId, connected, sessions]);
  const backgroundProjects = JSON.stringify(
    projects
      .filter(
        (p) =>
          (p.hostId !== hostId || p.id !== projectId) &&
          connectionStatus(p.hostId) === "connected",
      )
      .map((p) => ({
        hostId: p.hostId,
        id: p.id,
        connectionVersion: state.hostConnectionVersions?.[p.hostId] ?? 0,
      })),
  );
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      const targets: { hostId: string; id: string }[] =
        JSON.parse(backgroundProjects);
      for (const project of targets) {
        if (disposed) return;
        const key = `${project.hostId}:${project.id}`;
        try {
          const result = await request(project.hostId, {
            method: "sessions",
            project_id: project.id,
          });
          if (!disposed && result.kind === "sessions") {
            setSessionCache((old) => ({ ...old, [key]: result.sessions }));
          }
        } catch {
          // Keep the last known state; reconnect remains an explicit host action.
        }
      }
      if (!disposed) timer = setTimeout(() => void refresh(), 5000);
    };
    void refresh();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [backgroundProjects, connected, state.modal]);
  async function loadProjectSessions(project: HostedProject) {
    const key = `${project.hostId}:${project.id}`;
    setLoadingProjects((old) => ({ ...old, [key]: true }));
    setProjectErrors((old) => ({ ...old, [key]: "" }));
    try {
      const fetch = () =>
        request(project.hostId, {
          method: "sessions",
          project_id: project.id,
        });
      let result;
      try {
        result = await fetch();
      } catch {
        const host = hosts.find((value) => value.id === project.hostId);
        if (!host) throw new Error("项目所在主机已移除");
        await state.refreshHost(host);
        result = await fetch();
      }
      if (result.kind !== "sessions") throw new Error("无法加载项目任务");
      setSessionCache((old) => ({ ...old, [key]: result.sessions }));
    } catch (error) {
      setProjectErrors((old) => ({ ...old, [key]: message(error) }));
    } finally {
      setLoadingProjects((old) => ({ ...old, [key]: false }));
    }
  }
  useStatusIssues([
    ...hosts
      .filter((host) => connectionStatus(host.id) === "connected")
      .map((host): StatusIssue => {
        const data = recent.hosts[host.id];
        const unsupported = data?.errorKind === "unsupported";
        return {
          id: `recent:${host.id}`,
          title: `${host.name} 的${unsupported ? "完整历史暂不可用" : data?.errorKind === "more" ? "更早记录未能加载" : data?.errorKind === "refresh" ? "历史未能刷新" : "历史未能加载"}`,
          error: unsupported
            ? "连接正常，仍可从项目列表打开对话。当前服务版本不支持跨项目汇总历史；更新此主机上的服务并重新连接后会自动显示。"
            : (data?.error ?? ""),
          level: unsupported ? "warning" : "error",
          pending: data?.loading && sections.recent && visible,
          action: unsupported
            ? {
                label: "连接设置",
                run: () => {
                  state.setSettingsTab("ssh");
                  setModal("settings");
                },
              }
            : {
                label: sections.recent && visible ? "重试" : "查看最近",
                run: () => {
                  if (sections.recent && visible) recent.reload(host.id);
                  else {
                    setSections((old) => ({ ...old, recent: true }));
                    if (!visible) state.toggleSidebar();
                  }
                },
              },
        };
      }),
    ...projects
      .filter((p) => connectionStatus(p.hostId) === "connected")
      .map((p): StatusIssue => ({
        id: `project-history:${p.hostId}:${p.id}`,
        title: `${p.name} 的对话未能加载`,
        error: projectErrors[`${p.hostId}:${p.id}`] ?? "",
        pending: loadingProjects[`${p.hostId}:${p.id}`],
        action: { label: "重试", run: () => loadProjectSessions(p) },
      })),
  ]);
  const liveSessionState = (s: HostedSession) =>
    state.agentSessionState?.(s) ??
    agentSessionState(
      s,
      state.isHostConnected?.(s.hostId) ?? (s.hostId === hostId && connected),
    );
  const contextSession = sessionContext?.session;
  const menuSession = contextSession
    ? {
        ...((contextSession.hostId === hostId
          ? sessions.find((s) => s.id === contextSession.id)
          : undefined) ??
          state.pinned.find(
            (s) =>
              s.hostId === contextSession.hostId && s.id === contextSession.id,
          ) ??
          recentRows.find(
            (s) =>
              s.hostId === contextSession.hostId && s.id === contextSession.id,
          ) ??
          sessionCache[
            `${contextSession.hostId}:${contextSession.project_id}`
          ]?.find((s) => s.id === contextSession.id) ??
          contextSession),
        hostId: contextSession.hostId,
      }
    : null;
  const sessionRow = (
    s: HostedSession,
    list: "project" | "pinned" | "recent" = "project",
  ) => {
    const sourceProject = projects.find(
      (p) => p.hostId === s.hostId && p.id === s.project_id,
    );
    const sourceHost = hosts.find((h) => h.id === s.hostId);
    const duplicateTitle =
      list !== "project" &&
      (list === "pinned" ? state.pinned : recentRows).filter(
        (p) => p.title === s.title,
      ).length > 1;
    const showMenu = (point: { x: number; y: number }) => {
      setContext(null);
      setSessionContext({ session: s, point });
    };
    const loadingHistory =
      s.id === sessionId && s.hostId === hostId && loading.visible;
    const liveState = liveSessionState(s);
    const liveLabel = agentSessionLabels[liveState];
    const pendingAgent = liveState === "opening" || liveState === "restoring";
    const loadingLabel = loadingHistory
      ? "正在加载对话"
      : pendingAgent
        ? liveLabel
        : null;
    const row = (
      <SessionHoverCard
        title={s.title}
        project={sourceProject?.name ?? "项目"}
        host={sourceHost?.name ?? s.hostId}
        taskStatus={statusNames[s.status]}
        agentState={liveState}
      >
        <button
          className={`session-row ${list === "pinned" ? "pinned-session" : list === "recent" ? "recent-session" : ""} ${s.id === sessionId && s.hostId === hostId ? "active" : ""}`}
          aria-label={s.title}
          aria-description={`${sourceProject?.name ?? "项目"} · ${sourceHost?.name ?? s.hostId} · ${statusNames[s.status]}${liveState === "closed" ? "" : ` · ${liveLabel}`}`}
          onClick={() => {
            if (list === "project")
              setExpandedProjects((old) => ({
                ...old,
                [`${s.hostId}:${s.project_id}`]: true,
              }));
            state.openSession(s);
          }}
          onContextMenu={(e) => {
            e.preventDefault();
            e.currentTarget.focus();
            showMenu({ x: e.clientX, y: e.clientY });
          }}
          onKeyDown={(e) => {
            if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
              e.preventDefault();
              const rect = e.currentTarget.getBoundingClientRect();
              showMenu({ x: rect.left + 16, y: rect.bottom });
            }
          }}
        >
          <span className="session-label">
            {s.title}
            {duplicateTitle && (
              <small>
                {sourceProject?.name} · {sourceHost?.name}
              </small>
            )}
          </span>
          <span className="row-status">
            {loadingLabel ? (
              <span
                className="session-history-loading"
                aria-label={loadingLabel}
                title={loadingLabel}
              >
                <WorkingStatus label="" animated />
              </span>
            ) : s.status === "running" ? (
              <span
                className="session-progress"
                role="img"
                aria-label="执行中"
                title="执行中"
              >
                <i />
                <i />
                <i />
              </span>
            ) : s.status === "waiting" ? (
              <CirclePause
                className="session-waiting"
                size={14}
                role="img"
                aria-label="等待确认"
              >
                <title>等待确认</title>
              </CirclePause>
            ) : s.status === "failed" ? (
              <CircleAlert
                size={14}
                className="failure"
                role="img"
                aria-label="执行失败"
              >
                <title>执行失败</title>
              </CircleAlert>
            ) : s.status === "unknown" ? (
              <CircleHelp size={14} role="img" aria-label="状态待确认">
                <title>状态待确认</title>
              </CircleHelp>
            ) : s.unread ? (
              <span
                className="unread-dot"
                role="img"
                aria-label="未读"
                title="未读"
              />
            ) : list === "recent" && s.hostId !== "local" ? (
              <RemoteGlobeIcon className="recent-remote" />
            ) : null}
          </span>
        </button>
      </SessionHoverCard>
    );
    const menuOpen =
      sessionContext?.session.id === s.id &&
      sessionContext.session.hostId === s.hostId;
    const runAction = async (
      action: Parameters<Workbench["sessionAction"]>[1],
    ) => {
      setActionBusy(`${s.hostId}:${s.id}`);
      try {
        await state.sessionAction(s.hostId, action);
        recent.reload(s.hostId);
        if (sourceProject) await loadProjectSessions(sourceProject);
      } catch (error) {
        setError(message(error));
      } finally {
        setActionBusy(null);
      }
    };
    return (
      <div
        className="session-item"
        data-menu-open={menuOpen}
        key={`${s.hostId}:${s.id}`}
      >
        {row}
        <div className="session-row-actions">
          <button
            className="icon-button"
            aria-label={`${s.title} 的更多操作`}
            title="更多操作"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={(e) => {
              e.currentTarget.focus();
              const rect = e.currentTarget.getBoundingClientRect();
              showMenu({ x: rect.left, y: rect.bottom });
            }}
          >
            <MoreHorizontal size={14} />
          </button>
          <button
            className="icon-button"
            aria-label={`${s.pinned_at === null ? "置顶" : "取消置顶"} ${s.title}`}
            title={s.pinned_at === null ? "置顶" : "取消置顶"}
            disabled={!!actionBusy || s.archived}
            onClick={() =>
              void runAction({
                method: "pin_session",
                session_id: s.id,
                pinned: s.pinned_at === null,
              })
            }
          >
            {s.pinned_at === null ? <Pin size={14} /> : <PinOff size={14} />}
          </button>
          <button
            className="icon-button"
            aria-label={`${s.archived ? "取消归档" : "归档"} ${s.title}`}
            title={
              ["running", "waiting"].includes(s.status)
                ? "请先停止任务，再归档"
                : s.archived
                  ? "取消归档"
                  : "归档"
            }
            disabled={!!actionBusy || ["running", "waiting"].includes(s.status)}
            onClick={() =>
              void runAction({
                method: "archive_session",
                session_id: s.id,
                archived: !s.archived,
              })
            }
          >
            {s.archived ? <ArchiveRestore size={14} /> : <Archive size={14} />}
          </button>
        </div>
      </div>
    );
  };
  return (
    <aside
      id="project-sidebar"
      className={`sidebar${preview?.visible ? " sidebar-preview" : ""}`}
      hidden={!visible}
      ref={preview?.panel}
      {...preview?.panelProps}
      data-tauri-drag-region="deep"
    >
      <div className="window-drag" data-tauri-drag-region>
        {statusCenter}
        <button
          className="panel-toggle icon-button"
          data-sidebar-toggle
          title={preview?.visible ? "固定侧栏" : "收起侧栏"}
          aria-label={preview?.visible ? "固定侧栏" : "收起侧栏"}
          aria-expanded={true}
          aria-controls="project-sidebar"
          onClick={preview?.visible ? preview.pin : state.toggleSidebar}
        >
          <PanelLeft size={16} />
        </button>
      </div>
      <div className="brand">
        <div className="brand-mark" aria-hidden="true">
          L<span>••</span>
        </div>
        <span className="brand-name">
          Latte<span className="brand-light"> Work</span>
        </span>
      </div>
      <button
        className="new-task"
        title="新任务（⌘ N）"
        onClick={() => void createSession().catch((e) => setError(message(e)))}
      >
        <SquarePen size={16} />
        新任务<span>⌘ N</span>
      </button>
      <div className="sidebar-navigation">
        {state.pinned.length > 0 && (
          <section className="pinned-section" aria-label="置顶会话">
            <div className="section-heading">
              {sectionToggle("pinned", "置顶")}
            </div>
            <div
              id="sidebar-pinned"
              className="pinned-session-list"
              hidden={!sections.pinned}
            >
              {state.pinned.map((s) => sessionRow(s, "pinned"))}
            </div>
          </section>
        )}
        <div className="section-heading">
          {sectionToggle("projects", "项目")}
          <button
            title="添加项目"
            className="icon-button"
            onClick={(event) => {
              event.currentTarget.focus();
              openProject();
            }}
          >
            <Plus size={15} />
          </button>
        </div>
        <div
          id="sidebar-projects"
          className="project-list"
          hidden={!sections.projects}
        >
          {projects.map((p) => {
            const key = `${p.hostId}:${p.id}`;
            const selected = p.id === projectId && p.hostId === hostId;
            const expanded = expandedProjects[key] ?? false;
            const projectSessions = selected
              ? sessions
              : (sessionCache[key] ?? []);
            const listedSessions = projectSessions.filter(
              (s) =>
                !s.archived &&
                s.pinned_at === null &&
                !state.pinned.some(
                  (pinned) => pinned.hostId === p.hostId && pinned.id === s.id,
                ),
            );
            const showAll = showAllProjects[key] ?? false;
            const additionalSessionsId = `additional-sessions-${key}`;
            const allSessions = Array.from(
              new Map(
                [
                  ...state.pinned.filter(
                    (s) => s.hostId === p.hostId && s.project_id === p.id,
                  ),
                  ...projectSessions.map((s) => ({ ...s, hostId: p.hostId })),
                ].map((s) => [s.id, s]),
              ).values(),
            ).filter((s) => !s.archived);
            const hasSnapshot =
              (selected && connected) || sessionCache[key] !== undefined;
            const hostConnected =
              state.isHostConnected?.(p.hostId) ??
              (p.hostId === hostId && connected);
            const openStates = allSessions.map((s) => liveSessionState(s));
            const opened =
              hasSnapshot &&
              hostConnected &&
              allSessions.every((s) => s.agent_session_open !== undefined) &&
              openStates.every((s) => s === "open" || s === "closed")
                ? openStates.filter((s) => s === "open").length
                : null;
            const projectHost =
              hosts.find((h) => h.id === p.hostId)?.name ?? p.hostId;
            const connectionState = connectionStatus(p.hostId);
            const connectionLabel =
              connectionState === "connected"
                ? "已连接"
                : connectionState === "failed"
                  ? "连接失败"
                  : "正在连接";
            const noSessions =
              !projectSessions.some((s) => !s.archived) &&
              !state.pinned.some(
                (s) =>
                  s.hostId === p.hostId && s.project_id === p.id && !s.archived,
              );
            const empty =
              (selected ? connected : sessionCache[key] !== undefined) &&
              !loadingProjects[key] &&
              !projectErrors[key] &&
              noSessions;
            const running = listedSessions.some((s) => s.status === "running");
            const waiting = listedSessions.some((s) => s.status === "waiting");
            const unreadCompleted = listedSessions.some(
              (s) => s.status === "completed" && s.unread,
            );
            const toggle = () => {
              setExpandedProjects((old) => ({ ...old, [key]: !expanded }));
              if (!expanded && !selected) void loadProjectSessions(p);
            };
            const showProjectMenu = (point: { x: number; y: number }) => {
              setSessionContext(null);
              setContext({ project: p, point });
            };
            return (
              <div className="project-group" key={key}>
                <div
                  className={`project-heading ${selected ? "selected" : ""} ${empty ? "empty" : ""}`}
                  data-menu-open={
                    context?.project.id === p.id &&
                    context.project.hostId === p.hostId
                  }
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.currentTarget
                      .querySelector<HTMLButtonElement>(".project-row")
                      ?.focus();
                    const rect = e.currentTarget.getBoundingClientRect();
                    showProjectMenu({
                      x: e.clientX || rect.left + 20,
                      y: e.clientY || rect.bottom,
                    });
                  }}
                  onKeyDown={(e) => {
                    if (
                      e.key === "ContextMenu" ||
                      (e.shiftKey && e.key === "F10")
                    ) {
                      e.preventDefault();
                      const rect = e.currentTarget.getBoundingClientRect();
                      showProjectMenu({
                        x: rect.left + 20,
                        y: rect.bottom,
                      });
                    }
                  }}
                >
                  <ProjectHoverCard
                    title={p.name}
                    path={p.path}
                    host={projectHost}
                    total={hasSnapshot ? allSessions.length : null}
                    opened={opened}
                  >
                    <button
                      className="project-row"
                      onClick={toggle}
                      aria-label={p.name}
                      aria-expanded={expanded}
                      aria-controls={`sessions-${key}`}
                      aria-description={`${p.path} · ${projectHost}${p.hostId !== "local" ? ` · ${connectionLabel}` : ""}`}
                    >
                      <span className="project-folder">
                        {p.hostId !== "local" ? (
                          <RemoteProjectIcon
                            state={connectionState}
                            label={`${projectHost} ${connectionLabel}`}
                            title={`远程项目 · ${projectHost} · ${connectionLabel}${hostErrors[p.hostId] ? `：${hostErrors[p.hostId]}` : ""}`}
                          />
                        ) : expanded ? (
                          <FolderOpen size={15} />
                        ) : (
                          <Folder size={15} />
                        )}
                      </span>
                      <span className="project-name">{p.name}</span>
                      {p.hostId !== "local" && (
                        <small className="project-host">
                          <span className="project-host-name">
                            {projectHost}
                          </span>
                        </small>
                      )}
                      <span className="row-status">
                        {!expanded &&
                          (running ? (
                            <span
                              className="session-progress project-activity"
                              role="img"
                              aria-label="项目中有对话执行中"
                              title="有对话执行中"
                            >
                              <i />
                              <i />
                              <i />
                            </span>
                          ) : waiting ? (
                            <CirclePause
                              className="session-waiting project-activity"
                              size={14}
                              role="img"
                              aria-label="项目中有对话等待确认"
                            />
                          ) : unreadCompleted ? (
                            <span
                              className="unread-dot project-activity"
                              role="img"
                              aria-label="项目中有未读完成回复"
                              title="有已完成对话尚未阅读"
                            />
                          ) : null)}
                      </span>
                    </button>
                  </ProjectHoverCard>
                  <div className="project-row-actions">
                    <button
                      className="icon-button"
                      aria-label={`${p.name} 的更多操作`}
                      title="更多操作"
                      aria-haspopup="menu"
                      aria-expanded={
                        context?.project.id === p.id &&
                        context.project.hostId === p.hostId
                      }
                      onClick={(e) => {
                        e.currentTarget.focus();
                        const rect = e.currentTarget.getBoundingClientRect();
                        showProjectMenu({ x: rect.left, y: rect.bottom });
                      }}
                    >
                      <MoreHorizontal size={14} />
                    </button>
                    <button
                      className="project-new-task icon-button"
                      aria-label={`在 ${p.name} 中新建任务`}
                      title={`在 ${p.name} 中新建任务`}
                      onClick={() => {
                        setExpandedProjects((old) => ({ ...old, [key]: true }));
                        void createSession(p).catch((e) =>
                          setError(message(e)),
                        );
                      }}
                    >
                      <SquarePen size={14} />
                    </button>
                  </div>
                </div>
                <div id={`sessions-${key}`} hidden={!expanded}>
                  {expanded && (
                    <div className="session-list">
                      {listedSessions
                        .slice(0, PROJECT_SESSION_LIMIT)
                        .map((s) => sessionRow({ ...s, hostId: p.hostId }))}
                      {listedSessions.length > PROJECT_SESSION_LIMIT && (
                        <>
                          <div id={additionalSessionsId} hidden={!showAll}>
                            {showAll &&
                              listedSessions
                                .slice(PROJECT_SESSION_LIMIT)
                                .map((s) =>
                                  sessionRow({ ...s, hostId: p.hostId }),
                                )}
                          </div>
                          <button
                            className="project-sessions-toggle"
                            aria-label={`${showAll ? "收起显示" : "展开显示"} ${p.name} 的对话`}
                            aria-expanded={showAll}
                            aria-controls={additionalSessionsId}
                            onClick={() =>
                              setShowAllProjects((old) => ({
                                ...old,
                                [key]: !showAll,
                              }))
                            }
                          >
                            {showAll ? "收起显示" : "展开显示"}
                          </button>
                        </>
                      )}
                      {noSessions &&
                        !projectErrors[key] &&
                        !loadingProjects[key] && (
                          <span className="no-sessions">还没有任务</span>
                        )}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
          {projects.length === 0 && (
            <div className="sidebar-empty">
              <Folder size={22} />
              <p>把项目带到这里</p>
              <span>本地目录，或远程工作区</span>
            </div>
          )}
        </div>
        <section className="recent-section" aria-label="最近对话">
          <div className="section-heading">
            {sectionToggle("recent", "最近")}
          </div>
          <div id="sidebar-recent" hidden={!sections.recent}>
            {sections.recent && (
              <>
                {recentRows.map((session) => sessionRow(session, "recent"))}
                {recentRows.length === 0 &&
                  hosts.every(
                    (host) =>
                      connectionStatus(host.id) === "connected" &&
                      recent.hosts[host.id]?.loaded &&
                      !recent.hosts[host.id]?.error &&
                      !recent.hosts[host.id]?.next,
                  ) && <p className="recent-notice">暂无历史对话</p>}
                {hosts.map((host) => {
                  const data = recent.hosts[host.id];
                  const ready = connectionStatus(host.id) === "connected";
                  return (
                    <div key={host.id}>
                      {ready &&
                        data?.next &&
                        data.errorKind !== "unsupported" && (
                          <button
                            className="recent-notice"
                            disabled={data.loading}
                            onClick={() => recent.more(host.id)}
                          >
                            加载更早的对话
                            {hosts.length > 1 ? ` · ${host.name}` : ""}
                          </button>
                        )}
                    </div>
                  );
                })}
              </>
            )}
          </div>
        </section>
      </div>
      <div className="sidebar-footer">
        <button onClick={() => setModal("settings")}>
          <Settings2 size={16} />
          设置
        </button>
      </div>
      {sessionContext && menuSession && (
        <SessionActions
          key={`${sessionContext.session.hostId}:${sessionContext.session.id}`}
          session={menuSession}
          agentState={liveSessionState(menuSession)}
          point={sessionContext.point}
          state={{
            ...state,
            sessionAction: async (hostId, action) => {
              await state.sessionAction(hostId, action);
              recent.reload(hostId);
            },
          }}
          close={() => setSessionContext(null)}
        />
      )}
      {context && (
        <ProjectActions
          key={`${context.project.hostId}:${context.project.id}:${context.point.x}:${context.point.y}`}
          project={context.project}
          point={context.point}
          state={state}
          close={() => setContext(null)}
        />
      )}
    </aside>
  );
}
