import { LinkHandling } from "./LinkHandling";
import { TaskOverview } from "./TaskOverview";
import { useAppLifecycle } from "./appLifecycle";
import { StatusCenter } from "./StatusCenter";
import { useStatusNotices, type StatusNotice } from "./statusNotices";
import { useCallback, useRef, useState } from "react";
import { DraftStore } from "./drafts";
import { Folder, PanelRight, SquarePen } from "lucide-react";
import { request, message } from "./api";
import { Conversation } from "./Conversation";
import { Workspace } from "./Workspace";
import { Sidebar } from "./Sidebar";
import { SidebarToggle } from "./SidebarToggle";
import { SessionMenuButton } from "./SessionMenuButton";
import { useSidebarPreview } from "./useSidebarPreview";
import { SettingsPage } from "./SettingsPage";
import { HostDialogs } from "./HostDialogs";
import { SshPasswordDialog } from "./SshPasswordDialog";
import { useWorkbench } from "./useWorkbench";
import { useSubagents } from "./useSubagents";
import { latestTurnChanges } from "./transcript";
import {
  openWorkspaceResource,
  openTurnChanges,
  openSubagents,
  CONVERSATION_MIN_WIDTH,
} from "./workspaceState";

export default function App() {
  useAppLifecycle();
  const workbench = useWorkbench();
  const drafts = useRef(new DraftStore()).current;
  const historyScope = JSON.stringify([
    workbench.hostId,
    workbench.sessionId,
    workbench.viewRevision,
  ]);
  const [readyHistoryScope, setReadyHistoryScope] = useState<string | null>(
    null,
  );
  const historyReady = useCallback(
    () => setReadyHistoryScope(historyScope),
    [historyScope],
  );
  const historyPending =
    !!workbench.sessionId &&
    (workbench.connected || workbench.connecting) &&
    (workbench.historyLoading || readyHistoryScope !== historyScope);
  const {
    hostId,
    connected,
    connecting,
    agent,
    projectId,
    events,
    error,
    setError,
    panel,
    setPanel,
    sidebarOpen,
    toggleSidebar,
    leftWidth,
    rightWidth,
    host,
    project,
    session,
    send,
    resize,
    setRetry,
  } = workbench;
  const [sidebarInteraction, setSidebarInteraction] = useState(false);
  const [statusOpen, setStatusOpen] = useState(false);
  const sidebarPreview = useSidebarPreview({
    open: sidebarOpen,
    enabled: !workbench.modal && !workbench.passwordPrompt,
    interactionLocked: sidebarInteraction || statusOpen,
    toggle: toggleSidebar,
  });
  const subagents = useSubagents(hostId, session?.id, connected, events);
  const lastTurn = latestTurnChanges(events);
  const lastTurnVersion = lastTurn
    ? JSON.stringify([
        lastTurn.request_id,
        lastTurn.undo,
        lastTurn.interrupted,
        lastTurn.background_pending,
      ])
    : undefined;
  const showSubagents = () => openSubagents(workbench.workspaceId);
  const merged = panel && workbench.workspace.expanded;
  const statusNotices = workbench.hosts.flatMap<StatusNotice>((target) => {
    const detail = workbench.hostErrors[target.id];
    if (detail)
      return [
        {
          id: `connection:${target.id}`,
          level: "error" as const,
          title: `${target.name} 暂时无法连接`,
          detail,
          action: {
            label: "重新连接",
            run: () => workbench.refreshHost(target),
          },
        },
      ];
    if (workbench.hostConnectionStatus(target.id) === "connecting")
      return [
        {
          id: `connection:${target.id}`,
          level: "info" as const,
          title: `${target.name} 正在连接`,
          detail: "连接完成后会自动刷新项目和历史。",
        },
      ];
    return [];
  });
  if (error && connected && error !== workbench.hostErrors[hostId])
    statusNotices.push({
      id: `operation:${hostId}`,
      level: "error",
      title: "操作未完成",
      detail: error,
      action: { label: "知道了", run: () => setError("") },
    });
  if (connected && agent?.available === false)
    statusNotices.push({
      id: `agent:${hostId}`,
      level: "warning",
      title: `${agent.name} 暂不可用`,
      detail: agent.detail,
      action: {
        label: "连接与 Agent 设置",
        run: () => {
          workbench.setSettingsTab("agents");
          workbench.setModal("settings");
        },
      },
    });
  if (connected && subagents.error)
    statusNotices.push({
      id: `subagents:${hostId}:${session?.id}`,
      level: "warning",
      title: "子智能体记录暂不可用",
      detail: subagents.error,
      action: subagents.reload
        ? { label: "重试", run: subagents.reload }
        : undefined,
    });
  useStatusNotices(statusNotices);
  const sidebarVisible = sidebarOpen || sidebarPreview.visible;
  const statusCenter =
    !workbench.modal && !workbench.passwordPrompt ? (
      <StatusCenter onOpenChange={setStatusOpen} />
    ) : null;
  const collapsedStatusCenter = sidebarPreview.visible ? (
    <span className="titlebar-control-slot" aria-hidden="true" />
  ) : (
    statusCenter
  );
  const conversationHidden = merged && !workbench.workspace.conversationActive;
  const conversationActions =
    !workbench.modal && !workbench.passwordPrompt ? (
      <SessionMenuButton state={workbench} />
    ) : null;
  const overview = (
    <TaskOverview
      key={workbench.workspaceId}
      hostId={hostId}
      project={project}
      connected={connected}
      subagents={subagents}
      openChanges={() => openWorkspaceResource(workbench.workspaceId, "diff")}
      openSubagents={showSubagents}
    />
  );
  return (
    <LinkHandling workspaceId={workbench.workspaceId}>
      <div
        hidden={workbench.modal === "settings"}
        className={`app ${sidebarOpen ? "" : "sidebar-collapsed"}${panel ? " workspace-visible" : ""}${merged ? " workspace-merged" : ""}`}
        style={
          {
            "--sidebar-width": `${leftWidth}px`,
            "--conversation-min-width": `${CONVERSATION_MIN_WIDTH}px`,
            "--workspace-width": `${rightWidth}px`,
          } as React.CSSProperties
        }
      >
        <Sidebar
          state={workbench}
          historyPending={historyPending}
          preview={sidebarPreview}
          onInteractionChange={setSidebarInteraction}
          statusCenter={sidebarVisible ? statusCenter : null}
        />
        <div
          className="resize-handle"
          role="separator"
          hidden={!sidebarOpen}
          aria-label="调整侧栏宽度"
          onPointerDown={(e) => resize(e, "left")}
        />
        <main
          id="conversation-panel"
          className={`main${conversationHidden ? " conversation-hidden" : ""}`}
          role={merged ? "tabpanel" : undefined}
          aria-labelledby={merged ? "conversation-workspace-tab" : undefined}
          aria-hidden={conversationHidden || undefined}
          inert={conversationHidden}
        >
          <header
            className="topbar"
            data-tauri-drag-region="deep"
            inert={merged}
            aria-hidden={merged || undefined}
          >
            <div className="topbar-project">
              {!sidebarOpen && (
                <div className="topbar-controls">
                  {!merged && collapsedStatusCenter}
                  <SidebarToggle
                    toggle={toggleSidebar}
                    preview={sidebarPreview}
                  />
                  <button
                    className="panel-toggle icon-button"
                    title="新对话 (⌘ N)"
                    aria-label="新对话"
                    onClick={() =>
                      void workbench
                        .createSession()
                        .catch((e) => setError(message(e)))
                    }
                  >
                    <SquarePen size={16} />
                  </button>
                </div>
              )}
              {!sidebarOpen && (
                <span className="titlebar-divider" aria-hidden="true" />
              )}
              <div className="topbar-title">
                <Folder size={16} />
                <strong>{project?.name ?? "工作台"}</strong>
                <span className="topbar-slash">/</span>
                <span>{session?.title ?? "新任务"}</span>
              </div>
            </div>
            <div className="topbar-actions">
              {!merged && conversationActions}
              {overview}
              <span className="titlebar-divider" aria-hidden="true" />
              <button
                className="panel-toggle icon-button"
                title={panel ? "收起工作区" : "展开工作区"}
                aria-label={panel ? "收起工作区" : "展开工作区"}
                aria-expanded={panel}
                aria-controls="project-workspace"
                onClick={() => setPanel((v) => !v)}
              >
                <PanelRight size={16} />
              </button>
            </div>
          </header>
          <Conversation
            subagents={subagents}
            openSubagents={showSubagents}
            openTurnChanges={(changes, path) => {
              if (session)
                openTurnChanges(
                  workbench.workspaceId,
                  session.id,
                  changes.request_id,
                  changes.summary.baseline_at,
                  path,
                );
            }}
            drafts={drafts}
            agent={agent}
            session={session}
            hostId={hostId}
            agentSessionState={
              session
                ? workbench.agentSessionState({ ...session, hostId })
                : undefined
            }
            agentSessionError={
              session
                ? workbench.agentSessionTransitions[
                    JSON.stringify([hostId, session.id])
                  ]?.error
                : undefined
            }
            onModelReady={() =>
              setError((old) => (old.startsWith("模型不在当前") ? "" : old))
            }
            settingsOpen={workbench.modal === "settings" || conversationHidden}
            events={events}
            historyLoading={workbench.historyLoading}
            hasEarlierHistory={workbench.hasEarlierHistory}
            earlierHistoryLoading={workbench.earlierHistoryLoading}
            loadEarlierHistory={workbench.loadEarlierHistory}
            readingScope={workbench.readingScope}
            readingPositions={workbench.readingPositions}
            onHistoryReady={historyReady}
            connected={connected}
            connecting={connecting}
            connectionError={error}
            connectionLost={workbench.connectionLost}
            reconnect={() => setRetry((v) => v + 1)}
            available={agent?.available ?? false}
            projectName={project?.name}
            projectId={project?.id}
            project={project}
            projects={workbench.projects}
            hosts={workbench.hosts}
            selectTaskProject={(target) => {
              try {
                workbench.selectTaskProject(target);
              } catch (cause) {
                setError(message(cause));
              }
            }}
            openProject={workbench.openProject}
            send={send}
            cancel={async () => {
              if (!session) return false;
              try {
                await request(hostId, {
                  method: "cancel",
                  session_id: session.id,
                });
                return true;
              } catch (e) {
                setError(message(e));
                return false;
              }
            }}
            approve={async (id, allow) => {
              if (!session) return;
              try {
                await request(hostId, {
                  method: "approve",
                  session_id: session.id,
                  request_id: id,
                  allow,
                });
              } catch (e) {
                setError(message(e));
              }
            }}
          />
        </main>
        <div
          className="resize-handle workspace-resize"
          hidden={!panel || merged}
          role="separator"
          aria-label="调整工作区宽度"
          onPointerDown={(e) => resize(e, "right")}
        />
        <Workspace
          sessionId={session?.id}
          lastTurnVersion={lastTurnVersion}
          overview={overview}
          conversationActions={conversationActions}
          statusCenter={!sidebarOpen ? collapsedStatusCenter : null}
          subagents={subagents}
          workspaceId={workbench.workspaceId}
          conversationTitle={session?.title ?? "新任务"}
          sidebarOpen={sidebarOpen}
          toggleSidebar={toggleSidebar}
          sidebarPreview={sidebarPreview}
          hostId={hostId}
          hostName={host.name}
          project={project}
          visible={panel && workbench.modal !== "settings"}
          connected={connected}
          close={() =>
            merged ? workbench.restoreWorkspace() : setPanel(false)
          }
        />
        <HostDialogs state={workbench} />
      </div>
      <div hidden={workbench.modal !== "settings"}>
        <SettingsPage
          state={workbench}
          active={workbench.modal === "settings"}
        />
      </div>
      {workbench.passwordPrompt && (
        <SshPasswordDialog
          key={workbench.passwordPrompt.id}
          host={workbench.passwordPrompt}
          submit={workbench.submitPassword}
          cancel={workbench.cancelPasswordPrompt}
        />
      )}
    </LinkHandling>
  );
}
