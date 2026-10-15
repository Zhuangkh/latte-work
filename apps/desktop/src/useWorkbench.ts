import {
  HOST_DISCONNECTED_EVENT,
  HostConnectionError,
} from "./connectionErrors";
import {
  clampWorkspaceWidth,
  CONVERSATION_MIN_WIDTH,
  copyWorkspace,
  openSubagents,
  useWorkspaceState,
  workspaceKey,
} from "./workspaceState";
import { useEffect, useRef, useState } from "react";
import {
  isSubagentPanelCommand,
  appCommandAction,
  hasCommandArguments,
  subagentPanelArgumentNotice,
} from "./composerActions";
import {
  connect,
  disconnect,
  loadHosts,
  saveHosts,
  request,
  localHost,
  native,
  message,
  type Host,
} from "./api";
import type {
  AgentInfo,
  Effort,
  Event,
  Project,
  Session,
  Request,
  Response,
} from "./protocol";
import { appendEvents } from "./transcript";
import { LIVE_POLL_MS } from "./usePollingSnapshot";
import { HistoryCache } from "./historyCache";
import {
  agentSessionKey,
  agentSessionState,
  type AgentSessionTransition,
} from "./agentSessionState";
import { sessionAfterRefresh } from "./sessionSelection";
import {
  hostedProjects,
  readCatalog,
  writeCatalog,
  type HostedProject,
} from "./projectCatalog";
import {
  orderProjects,
  readProjectOrder,
  writeProjectOrder,
  moveProjectOrder,
} from "./projectOrder";

import {
  pinnedSessions,
  readPinnedCache,
  type HostedSession,
} from "./sessionNavigation";

type PasswordChallenge = {
  host: Host;
  promise: Promise<Response>;
  resolve: (response: Response) => void;
  reject: (error: Error) => void;
};

type HostInitialization = {
  hello: Extract<Response, { kind: "hello" }>;
  projects: Project[];
  reused: boolean;
};

function readSidebarPreference(): { open: boolean; width: number } {
  try {
    const value = JSON.parse(
      localStorage.getItem("latte-work.sidebar.v1") ?? "null",
    );
    return {
      open: typeof value?.open === "boolean" ? value.open : true,
      width:
        typeof value?.width === "number" && Number.isFinite(value.width)
          ? Math.max(200, Math.min(320, value.width))
          : 244,
    };
  } catch {
    return { open: true, width: 244 };
  }
}

/** Coordinates host transport and durable projections; UI components render this state. */
export function useWorkbench() {
  const [hosts, setHosts] = useState<Host[]>([localHost]);
  const [passwordPrompt, setPasswordPrompt] = useState<Host | null>(null);
  const passwordChallenges = useRef<PasswordChallenge[]>([]);
  function requestPassword(target: Host): Promise<Response> {
    const existing = passwordChallenges.current.find(
      (challenge) => challenge.host.id === target.id,
    );
    if (existing) return existing.promise;
    let resolve!: (response: Response) => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<Response>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    passwordChallenges.current.push({ host: target, promise, resolve, reject });
    if (passwordChallenges.current.length === 1) setPasswordPrompt(target);
    return promise;
  }
  function finishPasswordPrompt() {
    passwordChallenges.current.shift();
    setPasswordPrompt(passwordChallenges.current[0]?.host ?? null);
  }
  async function submitPassword(password: string) {
    const challenge = passwordChallenges.current[0];
    if (!challenge) return;
    const result = await connect(challenge.host, password);
    if (result.kind !== "hello")
      throw new Error(result.kind === "error" ? result.message : "连接失败");
    finishPasswordPrompt();
    challenge.resolve(result);
  }
  function cancelPasswordPrompt() {
    const challenge = passwordChallenges.current[0];
    if (!challenge) return;
    finishPasswordPrompt();
    challenge.reject(new Error("已取消 SSH 密码输入"));
  }
  function cachedHello(target: Host) {
    return liveHosts.current.has(target.id) &&
      helloTargets.current.get(target.id) === JSON.stringify(target)
      ? hostHellos.current.get(target.id)
      : undefined;
  }
  async function connectWithPrompt(
    target: Host,
    reuse = false,
    password?: string,
  ): Promise<Response> {
    const existing = reuse ? cachedHello(target) : undefined;
    if (existing) return existing;
    let result: Response;
    try {
      result = await connect(target, password);
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      if (target.auth === "password" && raw === "SSH_PASSWORD_REQUIRED")
        result = await requestPassword(target);
      else throw error;
    }
    if (result.kind === "hello") {
      hostHellos.current.set(target.id, result);
      helloTargets.current.set(target.id, JSON.stringify(target));
    }
    return result;
  }
  const [hostId, setHostId] = useState("local");
  const [retry, setRetry] = useState(0);
  const lastConnectionRetry = useRef(retry);
  const hostHellos = useRef(
    new Map<string, Extract<Response, { kind: "hello" }>>(),
  );
  const helloTargets = useRef(new Map<string, string>());
  const initializingHosts = useRef(
    new Map<string, Promise<HostInitialization>>(),
  );
  const refreshedHosts = useRef(new Set<string>());
  const historyCache = useRef(new HistoryCache()).current;
  const [connected, setConnected] = useState(false);
  const connectedHosts = useRef(new Set<string>());
  const liveHosts = useRef(new Set<string>());
  const lostHosts = useRef(new Set<string>());
  const seenServers = useRef(new Map<string, string>());
  const [restoreVersions, setRestoreVersions] = useState<
    Record<string, number>
  >({});
  const closedVersions = useRef(new Map<string, number>());
  const readingPositions = useRef(
    new Map<string, { top: number; follow: boolean }>(),
  );
  const [connecting, setConnecting] = useState(false);
  const [serverId, setServerId] = useState("");
  const [agent, setAgent] = useState<AgentInfo>();
  const [catalog, setCatalog] = useState(readCatalog);
  const [projectOrder, setProjectOrder] = useState(readProjectOrder);
  useEffect(() => writeProjectOrder(projectOrder), [projectOrder]);
  const [hostErrors, setHostErrors] = useState<Record<string, string>>({});
  const [hostConnectionVersions, setHostConnectionVersions] = useState<
    Record<string, number>
  >({});
  const projects = orderProjects(
    hostedProjects(
      catalog,
      hosts.map((h) => h.id),
    ),
    projectOrder,
  );
  function moveProject(source: string, target: string, after: boolean) {
    setProjectOrder((old) =>
      moveProjectOrder(old, projects, source, target, after),
    );
  }
  const [projectId, setProjectId] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionId, setSessionId] = useState("");
  const [draftWorkspaceId, setDraftWorkspaceId] = useState(() =>
    crypto.randomUUID(),
  );
  const workspaceId = workspaceKey(sessionId || `draft:${draftWorkspaceId}`);
  const [workspace, setWorkspace] = useWorkspaceState(workspaceId);
  const draft = useRef(true);
  const navigationRevision = useRef(0);
  const [viewRevision, setViewRevision] = useState(0);
  const sending = useRef(false);
  function resetConversationView() {
    navigationRevision.current += 1;
    setViewRevision(navigationRevision.current);
  }
  const selectedSessionId = useRef(sessionId);
  selectedSessionId.current = sessionId;
  const [pinCache, setPinCache] = useState(readPinnedCache);
  const pendingSession = useRef<{
    hostId: string;
    projectId: string;
    id: string;
  } | null>(null);
  const pinned = pinnedSessions(pinCache, projects, hostId, sessions);
  const pinnedHosts = JSON.stringify(
    hosts
      .filter(
        (target) => liveHosts.current.has(target.id) && !hostErrors[target.id],
      )
      .map((target) => ({
        id: target.id,
        version: hostConnectionVersions[target.id] ?? 0,
      })),
  );
  useEffect(() => {
    try {
      localStorage.setItem(
        "latte-work.pinned-sessions.v1",
        JSON.stringify(pinCache),
      );
    } catch {
      /* Display cache only. */
    }
  }, [pinCache]);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const timers = new Map<string, ReturnType<typeof setTimeout>>();
    const targets: { id: string }[] = JSON.parse(pinnedHosts);
    for (const target of targets) {
      const pollPins = async () => {
        try {
          const result = await request(target.id, {
            method: "pinned_sessions",
          });
          if (!disposed && result.kind === "sessions") {
            setPinCache((old) => ({ ...old, [target.id]: result.sessions }));
          }
        } catch {
          /* Host discovery/reconnect handles offline hosts. Keep cached shortcuts. */
        }
        if (!disposed)
          timers.set(
            target.id,
            setTimeout(() => void pollPins(), 5000),
          );
      };
      void pollPins();
    }
    return () => {
      disposed = true;
      timers.forEach((timer) => clearTimeout(timer));
    };
  }, [pinnedHosts]);
  const [events, setEvents] = useState<Event[]>([]);
  const [hasEarlierHistory, setHasEarlierHistory] = useState(false);
  const [earlierHistoryLoading, setEarlierHistoryLoading] = useState(false);
  const earlierLoads = useRef(new Set<string>());
  const [historyReadyScope, setHistoryReadyScope] = useState<string | null>(
    null,
  );
  const [error, setError] = useState("");
  const [admissionNotice, setAdmissionNotice] = useState<{
    host: string;
    session: string;
    text: string;
  }>();
  const [agentSessionTransitions, setAgentSessionTransitions] = useState<
    Record<string, AgentSessionTransition>
  >({});
  const openedEvidence = useRef(new Map<string, number>());
  const [modal, setModal] = useState<"project" | "settings" | null>(null);
  const [settingsTab, setSettingsTab] = useState<
    "agents" | "providers" | "ssh" | "appearance" | "archived" | "browser"
  >("agents");
  const [settingsReturnToProject, setSettingsReturnToProject] = useState(false);
  const [busy, setBusy] = useState(false);
  const [projectPath, setProjectPath] = useState("");
  const [projectName, setProjectName] = useState("");
  const [projectHostId, setProjectHostId] = useState("local");
  const panel = workspace.visible;
  const setPanel = (value: boolean | ((current: boolean) => boolean)) =>
    setWorkspace((state) => ({
      visible: typeof value === "function" ? value(state.visible) : value,
    }));
  const [sidebarOpen, setSidebarOpen] = useState(
    () => readSidebarPreference().open,
  );
  const [leftWidth, setLeftWidth] = useState(
    () => readSidebarPreference().width,
  );
  useEffect(() => {
    try {
      localStorage.setItem(
        "latte-work.sidebar.v1",
        JSON.stringify({ open: sidebarOpen, width: leftWidth }),
      );
    } catch {
      /* Nonessential layout preference. */
    }
  }, [sidebarOpen, leftWidth]);
  function toggleSidebar() {
    setSidebarOpen((open) => !open);
  }
  const [windowWidth, setWindowWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const update = () => setWindowWidth(window.innerWidth);
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  const rightWidth = clampWorkspaceWidth(
    workspace.width,
    windowWidth,
    sidebarOpen ? leftWidth : 0,
  );
  function restoreWorkspace() {
    const sidebarWidth = sidebarOpen ? leftWidth : 0;
    setWorkspace({
      visible: true,
      expanded: false,
      width: clampWorkspaceWidth(
        window.innerWidth - sidebarWidth - 320 - (sidebarOpen ? 2 : 1),
        window.innerWidth,
        sidebarWidth,
      ),
    });
  }
  const host = hosts.find((h) => h.id === hostId) ?? localHost;
  const project = projects.find(
    (p) => p.id === projectId && p.hostId === hostId,
  );
  const session = sessions.find((s) => s.id === sessionId);
  const selection = useRef({ hostId, projectId });
  selection.current = { hostId, projectId };
  useEffect(() => {
    const disconnected = (event: globalThis.Event) => {
      const failure = (event as unknown as CustomEvent<HostConnectionError>)
        .detail;
      lostHosts.current.add(failure.hostId);
      liveHosts.current.delete(failure.hostId);
      setHostErrors((old) => ({ ...old, [failure.hostId]: failure.message }));
      if (selection.current.hostId === failure.hostId) {
        setError(failure.message);
        setConnected(false);
      }
    };
    window.addEventListener(
      HOST_DISCONNECTED_EVENT,
      disconnected as EventListener,
    );
    return () =>
      window.removeEventListener(
        HOST_DISCONNECTED_EVENT,
        disconnected as EventListener,
      );
  }, []);
  const pendingSend = useRef<
    | {
        id: string;
        host: string;
        session: string;
        text: string;
        model: string | null;
        effort: Effort | null;
        permission_mode: string | null;
      }
    | undefined
  >(undefined);
  useEffect(() => {
    if (native)
      void loadHosts()
        .then(setHosts)
        .catch((e) => setError(message(e)));
  }, []);
  useEffect(() => writeCatalog(catalog), [catalog]);
  function updateProjects(id: string, values: Project[]) {
    setCatalog((old) => ({ ...old, [id]: values }));
  }
  function initializeHost(target: Host, force = false, password?: string) {
    const hello = !force && cachedHello(target);
    if (hello)
      return Promise.resolve({
        hello,
        projects: catalog[target.id] ?? [],
        reused: true,
      });
    const key = JSON.stringify(target);
    const pending = initializingHosts.current.get(key);
    if (pending && !force) return pending;
    const promise = (async (): Promise<HostInitialization> => {
      const hello = await connectWithPrompt(target, !force, password);
      if (hello.kind !== "hello")
        throw new Error(hello.kind === "error" ? hello.message : "连接失败");
      const result = await request(target.id, { method: "projects" });
      if (result.kind !== "projects") throw new Error("无法加载主机项目");
      setHostConnectionVersions((old) => ({
        ...old,
        [target.id]: (old[target.id] ?? 0) + 1,
      }));
      return { hello, projects: result.projects, reused: false };
    })().finally(() => {
      if (initializingHosts.current.get(key) === promise)
        initializingHosts.current.delete(key);
    });
    initializingHosts.current.set(key, promise);
    return promise;
  }
  async function refreshHost(target: Host, force = true) {
    try {
      const result = await initializeHost(target, force);
      connectedHosts.current.add(target.id);
      liveHosts.current.add(target.id);
      updateProjects(target.id, result.projects);
      setHostErrors((old) => ({ ...old, [target.id]: "" }));
      if (target.id === selection.current.hostId) {
        refreshedHosts.current.add(target.id);
        setRetry((v) => v + 1);
      }
    } catch (e) {
      liveHosts.current.delete(target.id);
      setHostErrors((old) => ({ ...old, [target.id]: message(e) }));
      if (target.id === selection.current.hostId) {
        setConnected(false);
        setError(message(e));
      }
    }
  }
  // Discover each saved host independently; one offline host cannot hide the others.
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    for (const target of hosts) {
      if (cachedHello(target)) continue;
      void initializeHost(target)
        .then((result) => {
          if (!disposed) {
            connectedHosts.current.add(target.id);
            liveHosts.current.add(target.id);
            updateProjects(target.id, result.projects);
            setHostErrors((old) => ({ ...old, [target.id]: "" }));
          }
        })
        .catch((e) => {
          if (!disposed)
            setHostErrors((old) => ({ ...old, [target.id]: message(e) }));
        });
    }
    return () => {
      disposed = true;
    };
  }, [hosts]);
  useEffect(() => {
    if (!native) {
      setError("请通过 make dev 启动桌面应用。浏览器预览不连接主机。");
      return;
    }
    let disposed = false;
    const force =
      lastConnectionRetry.current !== retry &&
      !refreshedHosts.current.delete(hostId);
    lastConnectionRetry.current = retry;
    const reuseConnection = !force && !!cachedHello(host);
    setConnecting(!reuseConnection);
    setConnected(reuseConnection);
    void initializeHost(host, force)
      .then(({ hello: r, projects: hostProjects, reused }) => {
        if (disposed) return;
        const oldServer = seenServers.current.get(host.id);
        if (oldServer && oldServer !== r.server_id)
          historyCache.clearHost(host.id);
        if (
          lostHosts.current.delete(host.id) ||
          (oldServer && oldServer !== r.server_id)
        ) {
          setRestoreVersions((old) => ({
            ...old,
            [host.id]: (old[host.id] ?? 0) + 1,
          }));
        }
        seenServers.current.set(host.id, r.server_id);
        setServerId(r.server_id);
        setAgent(r.agents.find((a) => a.id === "claude"));
        let missingProject = false;
        if (!reused) updateProjects(host.id, hostProjects);
        missingProject =
          selection.current.hostId === host.id &&
          !!selection.current.projectId &&
          !hostProjects.some((p) => p.id === selection.current.projectId);
        setProjectId((id) =>
          id
            ? hostProjects.some((p) => p.id === id)
              ? id
              : ""
            : (hostProjects[0]?.id ?? ""),
        );
        connectedHosts.current.add(host.id);
        liveHosts.current.add(host.id);
        setConnected(true);
        setHostErrors((old) => ({ ...old, [host.id]: "" }));
        setError(
          missingProject ? "所选项目已不在该主机上，请重新选择项目" : "",
        );
      })
      .catch((e) => {
        if (!disposed) {
          liveHosts.current.delete(host.id);
          setHostErrors((old) => ({ ...old, [host.id]: message(e) }));
          setError(message(e));
        }
      })
      .finally(() => {
        if (!disposed) setConnecting(false);
      });
    return () => {
      disposed = true;
    };
    // Host changes are keyed by immutable id; retry reconnects without dropping history.
  }, [hostId, retry]);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const timers = new Map<string, ReturnType<typeof setTimeout>>();
    for (const target of hosts) {
      const recover = async () => {
        if (disposed) return;
        if (
          !cachedHello(target) &&
          !initializingHosts.current.has(JSON.stringify(target))
        )
          await refreshHost(target, false);
        if (!disposed)
          timers.set(
            target.id,
            setTimeout(() => void recover(), 5000),
          );
      };
      timers.set(
        target.id,
        setTimeout(() => void recover(), 5000),
      );
    }
    return () => {
      disposed = true;
      timers.forEach((timer) => clearTimeout(timer));
    };
  }, [hosts]);
  useEffect(() => {
    if (!projectId || !connected) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const r = await request(hostId, {
          method: "sessions",
          project_id: projectId,
        });
        if (!disposed && r.kind === "sessions") {
          const pending = pendingSession.current;
          const desired =
            pending?.hostId === hostId && pending.projectId === projectId
              ? pending.id
              : null;
          if (desired && !r.sessions.some((s) => s.id === desired)) {
            const detail = await request(hostId, {
              method: "poll",
              session_id: desired,
              after: Number.MAX_SAFE_INTEGER,
            });
            if (disposed) return;
            if (
              detail.kind === "events" &&
              detail.session.project_id === projectId
            )
              r.sessions.push(detail.session);
          }
          if (desired) pendingSession.current = null;
          setSessions((old) => {
            const selected = old.find(
              (s) => s.id === selectedSessionId.current,
            );
            return selected && !r.sessions.some((s) => s.id === selected.id)
              ? [...r.sessions, selected]
              : r.sessions;
          });
          setSessionId((id) => sessionAfterRefresh(id, desired, draft.current));
        }
      } catch (e) {
        if (!disposed && !(e instanceof HostConnectionError))
          setError(message(e));
      }
      if (!disposed) timer = setTimeout(() => void refresh(), 5000);
    };
    void refresh();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [projectId, hostId, connected]);
  useEffect(() => {
    if (!sessionId || !connected) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const scope = JSON.stringify([hostId, sessionId]);
    const cached = historyCache.get(scope);
    let cursor = cached?.at(-1)?.seq ?? 0;
    let replaying = cached === undefined;
    let replay: Event[] = [];
    setHistoryReadyScope(cached ? scope : null);
    setEvents(cached ?? []);
    setHasEarlierHistory(historyCache.hasEarlier(scope));
    setEarlierHistoryLoading(false);
    const windowed = cachedHello(host)?.history_window === true;
    let initialWindow = replaying && windowed;
    let before: number | null = null;
    const poll = async () => {
      try {
        const r = await request(
          hostId,
          initialWindow
            ? {
                method: "history",
                session_id: sessionId,
                before,
              }
            : {
                method: "poll",
                session_id: sessionId,
                after: cursor,
              },
        );
        if (disposed) return;
        if (r.kind === "history") {
          const next = r.before;
          if (
            r.has_more &&
            (next === null || (before !== null && next >= before))
          )
            throw new Error("对话分页未前进，请重试");
          before = next ?? before;
          replay = appendEvents(r.events, replay);
          if (r.needs_earlier && r.has_more) {
            timer = setTimeout(() => void poll(), 0);
            return;
          }
          cursor = replay.at(-1)?.seq ?? 0;
          historyCache.set(scope, replay, r.has_more, r.before);
          setEvents(replay);
          setHasEarlierHistory(r.has_more);
          setHistoryReadyScope(scope);
          setSessions((previous) =>
            previous.map((s) => (s.id === r.session.id ? r.session : s)),
          );
          replay = [];
          initialWindow = false;
          replaying = false;
          timer = setTimeout(() => void poll(), LIVE_POLL_MS);
          return;
        }
        if (r.kind === "events") {
          cursor = r.events.at(-1)?.seq ?? cursor;
          if (replaying) {
            replay = appendEvents(replay, r.events);
            if (!r.has_more) {
              setEvents(replay);
              historyCache.set(scope, replay);
              replay = [];
              replaying = false;
              setHistoryReadyScope(scope);
            }
          } else if (r.events.length > 0) {
            setEvents((previous) => {
              const updated = appendEvents(previous, r.events);
              historyCache.set(
                scope,
                updated,
                historyCache.hasEarlier(scope),
                historyCache.before(scope),
              );
              return updated;
            });
          }
          if (
            r.session.unread &&
            !r.has_more &&
            !["running", "waiting"].includes(r.session.status) &&
            document.visibilityState === "visible" &&
            document.hasFocus()
          ) {
            try {
              const read = await request(hostId, {
                method: "mark_session_unread",
                session_id: sessionId,
                unread: false,
              });
              if (disposed) return;
              if (read.kind === "session") r.session = read.session;
            } catch {
              // Retry ordinary acknowledgement errors; transport failures notify the host owner.
            }
          }
          if (disposed) return;
          setSessions((previous) =>
            previous.map((s) => (s.id === r.session.id ? r.session : s)),
          );
          timer = setTimeout(() => void poll(), r.has_more ? 0 : LIVE_POLL_MS);
        }
      } catch (e) {
        if (!disposed && !(e instanceof HostConnectionError)) {
          setError(message(e));
          timer = setTimeout(() => void poll(), 5000);
        }
      }
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [sessionId, hostId, connected]);
  useEffect(() => {
    if (!native || !connected || !sessionId || !session || session.archived)
      return;
    const targetHost = hostId,
      targetId = sessionId;
    const key = agentSessionKey(targetHost, targetId);
    const closeVersion = closedVersions.current.get(key) ?? 0;
    const restoreVersion = restoreVersions[targetHost] ?? 0;
    if (
      session.agent_session_open === true &&
      openedEvidence.current.get(key) === restoreVersion
    )
      return;
    const needsRestore =
      openedEvidence.current.has(key) &&
      openedEvidence.current.get(key) !== restoreVersion;
    if (session.agent_session_open !== true || needsRestore)
      setAgentSessionTransitions((old) => ({
        ...old,
        [key]: {
          phase: openedEvidence.current.has(key) ? "restoring" : "opening",
        },
      }));
    void request(targetHost, {
      method: "open_agent_session",
      session_id: targetId,
    })
      .then(async () => {
        if ((closedVersions.current.get(key) ?? 0) !== closeVersion) return;
        const result = await request(targetHost, {
          method: "poll",
          session_id: targetId,
          after: Number.MAX_SAFE_INTEGER,
        });
        if ((closedVersions.current.get(key) ?? 0) !== closeVersion) return;
        if (
          result.kind !== "events" ||
          result.session.agent_session_open !== true
        )
          throw new Error(
            result.kind === "events" &&
              result.session.agent_session_open === undefined
              ? "当前 Server 未提供会话状态，请更新 Server"
              : "Agent 会话尚未打开，请重试",
          );
        openedEvidence.current.set(key, restoreVersion);
        updateSession(targetHost, result.session);
        setAgentSessionTransitions((old) => {
          const next = { ...old };
          delete next[key];
          return next;
        });
      })
      .catch((e) => {
        if ((closedVersions.current.get(key) ?? 0) !== closeVersion) return;
        setAgentSessionTransitions((old) => ({
          ...old,
          [key]: { phase: "error", error: message(e) },
        }));
      });
  }, [
    hostId,
    sessionId,
    connected,
    session?.archived,
    restoreVersions[hostId],
  ]);
  function updateSession(targetHostId: string, value: Session) {
    setPinCache((old) => ({
      ...old,
      [targetHostId]: [
        ...(old[targetHostId] ?? []).filter((s) => s.id !== value.id),
        ...(value.pinned_at !== null && !value.archived ? [value] : []),
      ],
    }));
    if (
      selection.current.hostId === targetHostId &&
      selection.current.projectId === value.project_id
    )
      setSessions((old) => old.map((s) => (s.id === value.id ? value : s)));
  }
  async function sessionAction(targetHostId: string, action: Request) {
    const result = await request(targetHostId, action);
    if (
      (action.method === "close_agent_session" && result.kind === "ok") ||
      (action.method === "archive_session" &&
        action.archived &&
        result.kind === "session" &&
        result.session.archived &&
        result.session.agent_session_open === false)
    ) {
      const key = JSON.stringify([targetHostId, action.session_id]);
      openedEvidence.current.delete(key);
      setAgentSessionTransitions((old) => {
        const next = { ...old };
        delete next[key];
        return next;
      });
      setSessions((old) =>
        selection.current.hostId === targetHostId
          ? old.map((s) =>
              s.id === action.session_id
                ? { ...s, agent_session_open: false, agent_session_busy: false }
                : s,
            )
          : old,
      );
      setPinCache((old) => ({
        ...old,
        [targetHostId]: (old[targetHostId] ?? []).map((s) =>
          s.id === action.session_id
            ? { ...s, agent_session_open: false, agent_session_busy: false }
            : s,
        ),
      }));
      closedVersions.current.set(
        key,
        (closedVersions.current.get(key) ?? 0) + 1,
      );
      if (
        selection.current.hostId === targetHostId &&
        selectedSessionId.current === action.session_id
      ) {
        draft.current = true;
        resetConversationView();
        setSessionId("");
        setEvents([]);
      }
    }
    if (result.kind === "session") updateSession(targetHostId, result.session);
    else if (result.kind !== "ok") throw new Error("会话操作失败");
  }
  function openSession(target: HostedSession) {
    if (target.hostId === hostId && target.id === sessionId) {
      if (target.unread && connected)
        void sessionAction(target.hostId, {
          method: "mark_session_unread",
          session_id: target.id,
          unread: false,
        }).catch((e) => setError(message(e)));
      return;
    }
    selectProject(target.hostId, target.project_id);
    draft.current = false;
    if (target.id !== sessionId) resetConversationView();
    pendingSession.current = {
      hostId: target.hostId,
      projectId: target.project_id,
      id: target.id,
    };
    if (target.hostId === hostId && target.project_id === projectId)
      pendingSession.current = null;
    else setSessions([target]);
    const scope = JSON.stringify([target.hostId, target.id]);
    const cached = historyCache.get(scope);
    setHasEarlierHistory(historyCache.hasEarlier(scope));
    setEarlierHistoryLoading(false);
    setHistoryReadyScope(cached ? scope : null);
    setEvents(cached ?? []);
    setSessionId(target.id);
  }
  async function loadEarlierHistory(): Promise<void> {
    const targetHost = hostId,
      targetSession = sessionId;
    const scope = JSON.stringify([targetHost, targetSession]);
    const cached = historyCache.get(scope);
    const before = historyCache.before(scope);
    const server = hostHellos.current.get(targetHost)?.server_id;
    if (
      !cached ||
      !before ||
      !historyCache.hasEarlier(scope) ||
      earlierLoads.current.has(scope)
    )
      return;
    earlierLoads.current.add(scope);
    setEarlierHistoryLoading(true);
    try {
      const r = await request(targetHost, {
        method: "history",
        session_id: targetSession,
        before,
      });
      if (hostHellos.current.get(targetHost)?.server_id !== server) return;
      if (r.kind !== "history") throw new Error("无法读取之前的对话记录");
      if (
        r.has_more &&
        (!r.events.length || r.before === null || r.before >= before)
      )
        throw new Error("对话分页未前进，请重试");
      const updated = appendEvents(r.events, historyCache.get(scope) ?? cached);
      historyCache.set(scope, updated, r.has_more, r.before);
      if (
        selection.current.hostId === targetHost &&
        selectedSessionId.current === targetSession
      ) {
        setEvents(updated);
        setHasEarlierHistory(r.has_more);
      }
    } catch (e) {
      if (
        selection.current.hostId === targetHost &&
        selectedSessionId.current === targetSession &&
        !(e instanceof HostConnectionError)
      )
        setError(message(e));
    } finally {
      earlierLoads.current.delete(scope);
      if (
        selection.current.hostId === targetHost &&
        selectedSessionId.current === targetSession
      )
        setEarlierHistoryLoading(false);
    }
  }
  async function createSession(target?: HostedProject): Promise<void> {
    if (target && !hosts.some((h) => h.id === target.hostId))
      throw new Error("项目所在主机已移除");
    pendingSession.current = null;
    draft.current = true;
    setDraftWorkspaceId(crypto.randomUUID());
    selectedSessionId.current = "";
    setSessionId("");
    setEvents([]);
    setError("");
    if (target && (target.hostId !== hostId || target.id !== projectId)) {
      setSessions([]);
      if (target.hostId !== hostId) {
        activateHost(target.hostId);
      }
      setProjectId(target.id);
    }
    resetConversationView();
  }
  function selectTaskProject(target: HostedProject) {
    const targetHostId = target.hostId;
    const id = target.id;
    if (!hosts.some((h) => h.id === targetHostId))
      throw new Error("项目所在主机已移除");
    if (targetHostId === hostId && id === projectId) return;
    pendingSession.current = null;
    draft.current = true;
    selectedSessionId.current = "";
    setSessionId("");
    setSessions([]);
    setEvents([]);
    setError("");
    if (targetHostId !== hostId) {
      activateHost(targetHostId);
    }
    setProjectId(id);
  }
  async function persistSession(
    revision: number,
  ): Promise<Session | undefined> {
    if (!project) return;
    const r = await request(hostId, {
      method: "create_session",
      project_id: project.id,
      agent: "claude",
    });
    if (r.kind === "session") {
      if (
        selection.current.hostId === hostId &&
        selection.current.projectId === project.id
      ) {
        setSessions((old) => [
          r.session,
          ...old.filter((s) => s.id !== r.session.id),
        ]);
        if (navigationRevision.current === revision) {
          copyWorkspace(workspaceId, workspaceKey(r.session.id));
          draft.current = false;
          selectedSessionId.current = r.session.id;
          setSessionId(r.session.id);
          setEvents([]);
        }
      }
      return r.session;
    }
  }
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest(".xterm"))
        return;
      if (
        (event.metaKey || event.ctrlKey) &&
        event.key.toLowerCase() === "n" &&
        !modal &&
        !document.querySelector('[role="dialog"]')
      ) {
        event.preventDefault();
        void createSession().catch((e) => setError(message(e)));
      }
      if (
        !event.defaultPrevented &&
        event.key === "Escape" &&
        !busy &&
        modal !== "settings"
      )
        setModal(null);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [modal, busy]);
  async function send(
    text: string,
    model: string | null,
    effort: Effort | null,
    permission_mode: string | null,
    uiAction?: "subagents",
  ): Promise<boolean> {
    if (project && (uiAction === "subagents" || isSubagentPanelCommand(text))) {
      openSubagents(workspaceId);
      if (hasCommandArguments(text)) {
        setError(subagentPanelArgumentNotice);
        return false;
      }
      setError("");
      return true;
    }
    const appAction = appCommandAction(text);
    if (appAction === "model" || appAction === "effort") {
      setError("请通过输入区的模型菜单选择模型或思考强度。");
      return false;
    }
    if (!connected || !text.trim() || sending.current) return false;
    sending.current = true;
    try {
      const target =
        session ?? (await persistSession(navigationRevision.current));
      if (!target) return false;
      const old = pendingSend.current;
      const id =
        old?.host === hostId &&
        old.session === target.id &&
        old.text === text &&
        old.model === model &&
        old.effort === effort &&
        old.permission_mode === permission_mode
          ? old.id
          : crypto.randomUUID();
      pendingSend.current = {
        id,
        host: hostId,
        session: target.id,
        text,
        model,
        effort,
        permission_mode,
      };
      const admission = await request(hostId, {
        method: "send_queued",
        session_id: target.id,
        request_id: id,
        text,
        model,
        effort,
        permission_mode,
      });
      if (admission.kind !== "accepted" && admission.kind !== "message_queued")
        throw new Error("消息未确认接收，请更新 Server 并核对状态");
      setAdmissionNotice(
        admission.duplicate
          ? {
              host: hostId,
              session: target.id,
              text: "已核对原请求，此消息此前已被接收，没有重复发送。",
            }
          : undefined,
      );
      if (pendingSend.current?.id === id) pendingSend.current = undefined;
      if (
        selection.current.hostId !== hostId ||
        selection.current.projectId !== projectId
      )
        return true;
      if (admission.kind === "accepted" && !admission.duplicate)
        setSessions((previous) =>
          previous.map((s) =>
            s.id === target.id
              ? { ...s, status: "running", model, effort, permission_mode }
              : s,
          ),
        );
      setError("");
      return true;
    } catch (e) {
      if (!(e instanceof HostConnectionError)) setError(message(e));
      return false;
    } finally {
      sending.current = false;
    }
  }
  async function addProject(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      const target = hosts.find((h) => h.id === projectHostId);
      if (!target) throw new Error("请选择项目所在主机");
      const initialization = await initializeHost(target);
      connectedHosts.current.add(target.id);
      liveHosts.current.add(target.id);
      updateProjects(target.id, initialization.projects);
      const r = await request(target.id, {
        method: "add_project",
        path: projectPath.trim(),
        name: projectName.trim() || null,
      });
      if (r.kind === "project") {
        setCatalog((old) => ({
          ...old,
          [target.id]: [
            ...(old[target.id] ?? []).filter((p) => p.id !== r.project.id),
            r.project,
          ],
        }));
        if (draft.current)
          selectTaskProject({ ...r.project, hostId: target.id });
        else selectProject(target.id, r.project.id);
        if (target.id === hostId && !connected) {
          refreshedHosts.current.add(target.id);
          setRetry((v) => v + 1);
        }
        setModal(null);
        setProjectPath("");
        setError("");
      }
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  async function saveSshHost(value: Host, password?: string) {
    const result = await initializeHost(value, true, password);
    const updated = hosts.some((host) => host.id === value.id)
      ? hosts.map((host) => (host.id === value.id ? value : host))
      : [...hosts, value];
    await saveHosts(updated);
    connectedHosts.current.add(value.id);
    liveHosts.current.add(value.id);
    updateProjects(value.id, result.projects);
    setHosts(updated);
    setHostErrors((old) => ({ ...old, [value.id]: "" }));
    setProjectHostId(value.id);
    if (value.id === hostId) {
      refreshedHosts.current.add(value.id);
      setRetry((count) => count + 1);
    }
  }
  async function removeSshHost(id: string) {
    const updated = hosts.filter((host) => host.id !== id);
    await saveHosts(updated);
    await disconnect(id);
    liveHosts.current.delete(id);
    connectedHosts.current.delete(id);
    hostHellos.current.delete(id);
    helloTargets.current.delete(id);
    seenServers.current.delete(id);
    lostHosts.current.delete(id);
    refreshedHosts.current.delete(id);
    historyCache.clearHost(id);
    setHosts(updated);
    setHostErrors((old) => {
      const next = { ...old };
      delete next[id];
      return next;
    });
    if (projectHostId === id) setProjectHostId("local");
    if (hostId === id) {
      activateHost("local");
      setProjectId("");
    }
  }
  function activateHost(id: string) {
    const target = hosts.find((value) => value.id === id);
    const hello = target ? cachedHello(target) : undefined;
    setConnected(!!hello);
    setAgent(hello?.agents.find((value) => value.id === "claude"));
    setServerId(hello?.server_id ?? "");
    setHostId(id);
  }
  function selectProject(targetHostId: string, id: string) {
    pendingSession.current = null;
    if (id === projectId && targetHostId === hostId) return;
    draft.current = false;
    resetConversationView();
    if (targetHostId !== hostId) {
      activateHost(targetHostId);
    }
    setError("");
    setProjectId(id);
    setSessions([]);
    setSessionId("");
    setEvents([]);
  }
  async function renameProject(targetHostId: string, id: string, name: string) {
    const result = await request(targetHostId, {
      method: "rename_project",
      project_id: id,
      name,
    });
    if (result.kind !== "project") throw new Error("项目更新失败");
    setCatalog((old) => ({
      ...old,
      [targetHostId]: (old[targetHostId] ?? []).map((p) =>
        p.id === id ? result.project : p,
      ),
    }));
  }
  async function removeProject(targetHostId: string, id: string) {
    const result = await request(targetHostId, {
      method: "remove_project",
      project_id: id,
    });
    if (result.kind !== "projects") throw new Error("项目移除失败");
    updateProjects(targetHostId, result.projects);
    if (
      selection.current.hostId === targetHostId &&
      selection.current.projectId === id
    )
      selectProject(targetHostId, result.projects[0]?.id ?? "");
  }
  function openProject() {
    setProjectHostId(hostId);
    setProjectPath("");
    setProjectName("");
    setError("");
    setModal("project");
  }
  const resizeCleanup = useRef<(() => void) | null>(null);
  useEffect(() => () => resizeCleanup.current?.(), []);
  function resize(e: React.PointerEvent<HTMLElement>, side: "left" | "right") {
    e.preventDefault();
    if (side === "right" && panel && workspace.expanded) return;
    resizeCleanup.current?.();
    const handle = e.currentTarget;
    handle.dataset.resizing = "true";
    const selectionStyle = document.body.style.userSelect;
    const webkitStyle = document.body.style.webkitUserSelect;
    document.body.style.userSelect = "none";
    document.body.style.webkitUserSelect = "none";
    window.getSelection()?.removeAllRanges();
    const x = e.clientX;
    const sidebarWidth = sidebarOpen ? leftWidth : 0;
    const merged = panel && workspace.expanded;
    const width = side === "left" ? leftWidth : rightWidth;
    let mergedDuringDrag = merged;
    e.currentTarget.setPointerCapture(e.pointerId);
    const move = (event: PointerEvent) => {
      const value = width + (event.clientX - x) * (side === "left" ? 1 : -1);
      if (side === "left")
        setLeftWidth(
          Math.max(
            200,
            Math.min(
              320,
              window.innerWidth -
                (panel && !merged ? rightWidth : 0) -
                CONVERSATION_MIN_WIDTH -
                (panel ? 2 : 1),
              value,
            ),
          ),
        );
      else {
        if (mergedDuringDrag) return;
        const splitWidth = clampWorkspaceWidth(
          value,
          window.innerWidth,
          sidebarWidth,
        );
        if (event.clientX <= sidebarWidth + 24) {
          mergedDuringDrag = true;
          setWorkspace({ expanded: true, conversationActive: true });
        } else {
          setWorkspace({ width: splitWidth });
        }
      }
    };
    const stop = () => {
      delete handle.dataset.resizing;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      document.body.style.userSelect = selectionStyle;
      document.body.style.webkitUserSelect = webkitStyle;
      resizeCleanup.current = null;
    };
    window.addEventListener("pointermove", move);
    resizeCleanup.current = stop;
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  }
  return {
    hosts,
    passwordPrompt,
    admissionNotice:
      admissionNotice?.host === hostId && admissionNotice.session === sessionId
        ? admissionNotice.text
        : undefined,
    dismissAdmissionNotice: () => setAdmissionNotice(undefined),
    submitPassword,
    cancelPasswordPrompt,
    hostId,
    setHostId,
    connected,
    hostPathSyntax: (id: string) => {
      const target = hosts.find((host) => host.id === id);
      return target ? cachedHello(target)?.path_syntax : undefined;
    },
    hostSupportsTerminal: (id: string) => {
      const hello = hostHellos.current.get(id);
      return !hello || hello.terminal !== false;
    },
    hostSupportsUndo: (id: string) => {
      const hello = hostHellos.current.get(id);
      return !hello || hello.undo !== false;
    },
    connectionLost: connectedHosts.current.has(hostId),
    agentSessionTransitions,
    agentSessionState: (target: HostedSession) =>
      agentSessionState(
        target,
        (target.hostId === hostId
          ? connected
          : liveHosts.current.has(target.hostId)) && !hostErrors[target.hostId],
        agentSessionTransitions[agentSessionKey(target.hostId, target.id)],
        target.hostId === hostId && target.id === sessionId,
      ),
    retryAgentSession: () => setRetry((v) => v + 1),
    isHostConnected: (id: string) =>
      (id === hostId ? connected : liveHosts.current.has(id)) &&
      !hostErrors[id],
    hostConnectionStatus: (
      id: string,
    ): "connected" | "failed" | "connecting" =>
      hostErrors[id]
        ? "failed"
        : liveHosts.current.has(id)
          ? "connected"
          : "connecting",
    hostConnectionVersions,
    connecting,
    serverId,
    agent,
    projects,
    moveProject,
    hostErrors,
    projectId,
    sessions,
    pinned,
    openSession,
    sessionAction,
    sessionId,
    workspaceId,
    workspace,
    viewRevision,
    readingPositions: readingPositions.current,
    readingScope: JSON.stringify([
      hostId,
      sessionId,
      closedVersions.current.get(JSON.stringify([hostId, sessionId])) ?? 0,
      restoreVersions[hostId] ?? 0,
    ]),
    setSessionId,
    events,
    hasEarlierHistory,
    earlierHistoryLoading,
    loadEarlierHistory,
    historyLoading:
      !!sessionId && historyReadyScope !== JSON.stringify([hostId, sessionId]),
    setEvents,
    error,
    setError,
    modal,
    setModal,
    settingsTab,
    setSettingsTab,
    settingsReturnToProject,
    setSettingsReturnToProject,
    busy,
    projectPath,
    setProjectPath,
    projectName,
    setProjectName,
    projectHostId,
    setProjectHostId,
    openProject,
    renameProject,
    removeProject,
    refreshHost,
    saveSshHost,
    removeSshHost,
    panel,
    setPanel,
    restoreWorkspace,
    sidebarOpen,
    toggleSidebar,
    leftWidth,
    rightWidth,
    host,
    project,
    session,
    createSession,
    selectTaskProject,
    send,
    addProject,
    selectProject,
    resize,
    setRetry,
  };
}
export type Workbench = ReturnType<typeof useWorkbench>;
