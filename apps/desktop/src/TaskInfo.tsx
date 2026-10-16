import {
  useCallback,
  useId,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  Bot,
  Check,
  Circle,
  CircleDashed,
  GitCompareArrows,
  ListTodo,
  FolderInput,
  Package,
  RefreshCw,
  History,
  Sparkles,
  Wrench,
  ChevronRight,
} from "lucide-react";
import { request } from "./api";
import type { Event, GitReview, Project } from "./protocol";
import { TaskLinkRow } from "./TaskLinkRow";
import { useBrowserHistory } from "./browserHistory";
import { webLink } from "./browserLinks";
import { TaskFileRow } from "./TaskFileRow";
import { SkillRow } from "./SkillRow";
import { createTaskInfoReader } from "./taskInfoModel";
import { SubagentsPanel } from "./SubagentsPanel";
import type { SubagentsState } from "./useSubagents";
import { openSubagent, openWorkspaceResource } from "./workspaceState";
import { usePollingSnapshot } from "./usePollingSnapshot";
import { useStatusIssues } from "./statusNotices";
import { useTaskDisclosure } from "./useTaskDisclosure";

function Section({
  title,
  icon,
  children,
  action,
  note,
  count,
  scope,
}: {
  title: string;
  icon: ReactNode;
  children: ReactNode;
  action?: ReactNode;
  note?: ReactNode;
  count?: number;
  scope?: string;
}) {
  const id = useId();
  const [open, toggle] = useTaskDisclosure(scope, title);
  return (
    <section
      className="task-info-section"
      aria-label={title}
      data-collapsed={!open}
    >
      <h2>
        <button
          type="button"
          className="task-info-toggle"
          aria-label={title}
          aria-expanded={open}
          aria-controls={id}
          aria-describedby={count === undefined ? undefined : `${id}-count`}
          onClick={toggle}
        >
          {icon}
          <span>{title}</span>
          {count !== undefined && <small id={`${id}-count`}>{count}</small>}
          <ChevronRight size={14} className="disclosure-chevron" />
        </button>
        {note}
        {action}
      </h2>
      <div id={id} hidden={!open}>
        {children}
      </div>
    </section>
  );
}
const empty = (value: string) => <p className="task-info-empty">{value}</p>;
const ignorePageTitleError = () => {};
const emptyInfo = createTaskInfoReader()([]);
const todoLabels: Record<string, string> = {
  pending: "待处理",
  in_progress: "进行中",
  completed: "已完成",
};

export function TaskInfo({
  workspaceId,
  active,
  hostId,
  project,
  sessionId,
  connected,
  events,
  subagents,
  running = false,
  historyLoading = false,
  hasEarlierHistory = false,
  earlierHistoryLoading = false,
  loadEarlierHistory,
}: {
  workspaceId: string;
  active: boolean;
  hostId: string;
  project?: Project;
  sessionId?: string;
  connected: boolean;
  events: Event[];
  subagents: SubagentsState;
  running?: boolean;
  historyLoading?: boolean;
  hasEarlierHistory?: boolean;
  earlierHistoryLoading?: boolean;
  loadEarlierHistory?: () => Promise<void>;
}) {
  const scope = JSON.stringify([hostId, project?.id, project?.path, sessionId]);
  const disclosureScope = sessionId
    ? JSON.stringify(["summary", hostId, sessionId])
    : undefined;
  const [revision, refresh] = useState(0);
  const readInfo = useMemo(() => createTaskInfoReader(), [scope]);
  const [info, setInfo] = useState({ scope: "", data: emptyInfo });
  // Only committed renders may advance the incremental reader. A suspended or
  // abandoned render must not change the cache shared with the visible pane.
  useLayoutEffect(() => {
    setInfo({
      scope,
      data: readInfo(events, sessionId, project?.path, !running),
    });
  }, [scope, readInfo, events, sessionId, project?.path, running]);
  const data = info.scope === scope ? info.data : emptyInfo;
  const visits = useBrowserHistory(ignorePageTitleError, active);
  const pageTitles = useMemo(() => {
    const titles = new Map<string, string>();
    for (const visit of visits) {
      const address = new URL(visit.url);
      const title = visit.title.replace(/\s+/g, " ").trim();
      // A newly opened page initially carries only its hostname, not a title.
      if (
        !title ||
        webLink(title) === visit.url ||
        title.toLowerCase() === address.hostname.toLowerCase()
      )
        continue;
      titles.set(address.href, title);
      address.hash = "";
      titles.set(address.href, title);
    }
    return titles;
  }, [visits]);
  const linkTitle = (url: string, fallback: string) => {
    const address = new URL(url);
    address.hash = "";
    return (
      pageTitles.get(url) ||
      pageTitles.get(address.href) ||
      (webLink(fallback) === url ? address.host : fallback)
    );
  };
  const version = events
    .filter(
      (entry) =>
        entry.session_id === sessionId &&
        ["user", "tool", "tool_result", "turn_changes", "state"].includes(
          entry.event.kind,
        ),
    )
    .at(-1)?.seq;
  const readChanges = useCallback(async () => {
    const response = await request(hostId, {
      method: "git_review",
      project_id: project!.id,
      scope: "branch",
      base: null,
    });
    if (response.kind !== "git_review")
      throw new Error("无法读取变更，请检查 Server 版本");
    return response.review;
  }, [hostId, project?.id]);
  const readSources = useCallback(async () => {
    if (!sessionId) return undefined;
    const response = await request(hostId, {
      method: "sources",
      session_id: sessionId,
    });
    if (response.kind !== "sources")
      throw new Error("无法读取工具列表，请更新 Server");
    return response;
  }, [hostId, sessionId]);
  const enabled = active && connected && !!project;
  // Branch diffs are read on opening, explicit refresh and settlement. Tool
  // activity during a turn never spawns a Git status/diff per streamed update.
  const changes = usePollingSnapshot<GitReview | undefined>(
    scope,
    enabled,
    readChanges,
    undefined,
    `${revision}:${running ? "running" : version}`,
    0,
    0,
  );
  const sources = usePollingSnapshot(
    scope,
    enabled,
    readSources,
    undefined,
    `${revision}:${running ? "running" : version}`,
    running ? 2000 : 0,
    0,
  );
  const current = {
    review: changes.data,
    sources: sources.data?.entries,
    truncated: sources.data?.truncated,
    changesError: changes.error,
    sourcesError: sources.error,
    loading: changes.loading || sources.loading,
  };
  useStatusIssues([
    {
      id: `task-info-changes:${scope}`,
      title: "任务摘要变更暂不可用",
      pending: active && connected && current.loading,
      error: active && connected ? (current.changesError ?? "") : "",
      action: { label: "重试", run: () => refresh((value) => value + 1) },
    },
    {
      id: `task-info-sources:${scope}`,
      title: "任务工具列表暂不可用",
      pending: active && connected && current.loading,
      error: active && connected ? (current.sourcesError ?? "") : "",
      action: { label: "重试", run: () => refresh((value) => value + 1) },
    },
  ]);
  const review = current.review;
  const tools = current.sources?.filter(
    (source) => source.kind === "tool" || source.kind === "connector",
  );
  const recordedEmpty = historyLoading ? "正在读取…" : "暂无记录";
  const refreshButton = (
    <button
      className="icon-button"
      aria-label="刷新任务摘要"
      title="刷新任务摘要"
      disabled={!connected || current.loading || !project}
      onClick={() => {
        refresh((value) => value + 1);
        subagents.reload?.();
      }}
    >
      <RefreshCw size={16} />
    </button>
  );
  return (
    <div className="task-info">
      {!connected && empty("主机未连接，信息待更新")}
      <Section
        title="变更概览"
        icon={<GitCompareArrows size={16} />}
        action={refreshButton}
        scope={disclosureScope}
        count={review?.entries.length}
      >
        <button
          className="task-info-row"
          disabled={!project}
          onClick={() => openWorkspaceResource(workspaceId, "diff")}
        >
          <span>分支变更</span>
          <span className="task-info-meta">
            {!connected
              ? "状态待确认"
              : current.changesError
                ? "暂不可用"
                : !review
                  ? project
                    ? "读取中…"
                    : "请先选择项目"
                  : `${review.entries.length} 个文件`}
          </span>
        </button>
        {review && (
          <div className="overview-change-stats">
            <span className="change-totals">
              <b>+{review.added}</b>
              <em>-{review.removed}</em>
            </span>
            <small>{review.truncated ? "部分记录" : "分支"}</small>
          </div>
        )}
      </Section>
      <Section
        title="子智能体"
        icon={<Bot size={16} />}
        scope={disclosureScope}
        count={
          subagents.loading && !subagents.tasks.length
            ? undefined
            : subagents.tasks.length
        }
      >
        {subagents.tasks.length ? (
          <SubagentsPanel
            state={subagents}
            hostId={hostId}
            sessionId={sessionId}
            active={active}
            connected={connected}
            onOpen={(task) => openSubagent(workspaceId, task)}
          />
        ) : (
          empty(
            subagents.loading
              ? "正在读取…"
              : subagents.error
                ? "暂不可用"
                : "暂无记录",
          )
        )}
      </Section>
      <Section
        title="待办事项"
        icon={<ListTodo size={16} />}
        scope={disclosureScope}
        count={historyLoading ? undefined : data.todos.length}
      >
        {!data.todos.length && empty(recordedEmpty)}
        <ul className="task-info-list">
          {data.todos.map((todo) => {
            const Icon =
              todo.status === "completed"
                ? Check
                : todo.status === "in_progress"
                  ? CircleDashed
                  : Circle;
            return (
              <li key={todo.id} className="task-info-todo">
                <Icon size={16} />
                <span>{todo.title}</span>
                <small>
                  {todo.status === "in_progress" && !connected
                    ? "状态待确认"
                    : todoLabels[todo.status]}
                </small>
              </li>
            );
          })}
        </ul>
      </Section>
      <Section
        title="调用的 Skill"
        icon={<Sparkles size={16} />}
        scope={disclosureScope}
        count={historyLoading ? undefined : data.skills.length}
      >
        {!data.skills.length && empty(recordedEmpty)}
        <ul className="task-info-list">
          {data.skills.map(([name, uses]) => (
            <li key={name}>
              <SkillRow
                name={name}
                uses={uses}
                workspaceId={workspaceId}
                hostId={hostId}
                sessionId={sessionId}
                project={project}
                connected={connected}
                active={active}
              />
            </li>
          ))}
        </ul>
      </Section>
      <Section
        title="调用的 Tools"
        icon={<Wrench size={16} />}
        scope={disclosureScope}
        count={
          tools?.length ?? (historyLoading ? undefined : data.tools.length)
        }
        note={
          current.truncated || (!tools && data.tools.length > 0) ? (
            <small title="仅包含已读取的工具调用">部分记录</small>
          ) : undefined
        }
      >
        {tools ? (
          tools.length ? (
            <ul className="task-info-list">
              {tools.map((tool) => (
                <li key={tool.id}>
                  <div className="task-info-row">
                    <span>{tool.name}</span>
                    <small>{tool.uses} 次</small>
                  </div>
                  {tool.kind === "connector" && (
                    <p className="task-info-tool-names">
                      {tool.tools.join(" · ")}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            empty("暂无记录")
          )
        ) : data.tools.length ? (
          <>
            <ul className="task-info-list">
              {data.tools.map(([name, uses]) => (
                <li className="task-info-row" key={name}>
                  <span>{name}</span>
                  <small>{uses} 次</small>
                </li>
              ))}
            </ul>
          </>
        ) : (
          empty(
            current.sourcesError
              ? "暂不可用"
              : sessionId && current.loading
                ? "正在读取…"
                : recordedEmpty,
          )
        )}
      </Section>
      {(
        [
          ["任务产物", data.products, <Package size={16} />],
          ["任务资源", data.resources, <FolderInput size={16} />],
        ] as const
      ).map(([title, entries, icon]) => (
        <Section
          key={title}
          title={title}
          icon={icon}
          scope={disclosureScope}
          count={historyLoading ? undefined : entries.length}
        >
          {!entries.length && empty(recordedEmpty)}
          <ul className="task-info-list">
            {entries.map((entry) =>
              entry.kind === "link" ? (
                <li key={`link:${entry.url}`}>
                  <TaskLinkRow
                    url={entry.url}
                    title={linkTitle(entry.url, entry.title)}
                  />
                </li>
              ) : (
                <li key={`file:${entry.path}`}>
                  <TaskFileRow
                    file={entry}
                    workspaceId={workspaceId}
                    hostId={hostId}
                    project={project}
                    sessionId={sessionId}
                    active={active}
                    connected={connected}
                  />
                </li>
              ),
            )}
          </ul>
        </Section>
      ))}
      {hasEarlierHistory && loadEarlierHistory && (
        <button
          className="task-info-earlier"
          title="当前仅包含已加载的对话记录，点击读取更早记录"
          disabled={!connected || historyLoading || earlierHistoryLoading}
          onClick={() => void loadEarlierHistory()}
        >
          <History size={14} />
          {earlierHistoryLoading ? "正在读取…" : "查看更早记录"}
        </button>
      )}
      {data.truncated && empty("各项最多显示 128 条记录")}
    </div>
  );
}
