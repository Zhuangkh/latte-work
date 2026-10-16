import { webLink } from "./browserLinks";
import { useSyncExternalStore } from "react";
import type { FileEntry, Project, TerminalInfo, Subagent } from "./protocol";

export type WorkspaceTab = {
  hostId?: string;
  hostName?: string;
  project?: Project;
} & (
  | {
      id: string;
      kind: "files" | "diff" | "subagents" | "taskInfo";
      requestId?: string;
      snapshotSessionId?: string;
      snapshotAt?: number | null;
      path?: string;
      file?: string;
      reference?: FileEntry;
      sessionId?: string;
    }
  | {
      id: string;
      kind: "turnChanges";
      requestId: string;
      sessionId: string;
      title: string;
      file?: string;
    }
  | {
      id: string;
      kind: "artifact";
      sessionId?: string;
      path: string;
      name: string;
      directory: boolean;
    }
  | { id: string; kind: "subagent"; taskId: string; title: string }
  | { id: string; kind: "browser"; url: string; title: string }
  | { id: string; kind: "terminal"; terminal: TerminalInfo }
);
export interface WorkspaceState {
  tabs: WorkspaceTab[];
  current: string;
  visible: boolean;
  expanded: boolean;
  conversationActive: boolean;
  width: number;
}
export const CONVERSATION_MIN_WIDTH = 240;
export const WORKSPACE_MIN_WIDTH = 280;
const RESIZE_HANDLE_WIDTH = 1;
export function clampWorkspaceWidth(
  width: number,
  windowWidth: number,
  sidebarWidth: number,
) {
  const handles = sidebarWidth > 0 ? 2 : 1;
  const maximum = Math.max(
    WORKSPACE_MIN_WIDTH,
    windowWidth -
      sidebarWidth -
      CONVERSATION_MIN_WIDTH -
      handles * RESIZE_HANDLE_WIDTH,
  );
  return Math.max(WORKSPACE_MIN_WIDTH, Math.min(width, maximum));
}
const prefix = "latte-work.conversation-workspace.v1:";
const cache = new Map<string, WorkspaceState>();
const listeners = new Set<() => void>();
export function workspaceKey(conversation: string) {
  return conversation;
}
export function readWorkspace(key: string): WorkspaceState {
  const cached = cache.get(key);
  if (cached) return cached;
  let state: WorkspaceState = {
    tabs: [],
    current: "",
    visible: false,
    expanded: false,
    conversationActive: false,
    width: 350,
  };
  try {
    let serialized = localStorage.getItem(prefix + key);
    // Preserve preview state written by the earlier host/project-scoped build.
    if (serialized === null) {
      for (let i = 0; i < localStorage.length; i++) {
        const oldKey = localStorage.key(i);
        if (!oldKey?.startsWith(prefix + "[")) continue;
        try {
          const scope = JSON.parse(oldKey.slice(prefix.length));
          if (Array.isArray(scope) && scope.length === 3 && scope[2] === key) {
            serialized = localStorage.getItem(oldKey);
            if (serialized !== null)
              localStorage.setItem(prefix + key, serialized);
            break;
          }
        } catch {
          /* Ignore unrelated or invalid legacy entries. */
        }
      }
    }
    const saved = JSON.parse(serialized ?? "null");
    if (saved && Array.isArray(saved.tabs)) {
      const ids = new Set<string>();
      let tabs: WorkspaceTab[] = saved.tabs.filter((tab: WorkspaceTab) => {
        if (!tab || typeof tab.id !== "string" || ids.has(tab.id)) return false;
        const valid =
          tab.kind === "files" ||
          tab.kind === "diff" ||
          tab.kind === "subagents" ||
          tab.kind === "taskInfo"
            ? (tab.path === undefined || typeof tab.path === "string") &&
              (tab.file === undefined || typeof tab.file === "string") &&
              (tab.reference === undefined ||
                (typeof tab.reference.path === "string" &&
                  !!tab.reference.path &&
                  typeof tab.reference.name === "string" &&
                  typeof tab.reference.directory === "boolean")) &&
              (tab.kind !== "diff" ||
                tab.requestId === undefined ||
                (typeof tab.requestId === "string" &&
                  !!tab.requestId &&
                  tab.requestId.length <= 256 &&
                  typeof tab.snapshotSessionId === "string" &&
                  !!tab.snapshotSessionId))
            : tab.kind === "artifact"
              ? typeof tab.path === "string" &&
                !!tab.path &&
                typeof tab.name === "string" &&
                typeof tab.directory === "boolean"
              : tab.kind === "browser"
                ? !!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
                    tab.id,
                  ) &&
                  typeof tab.title === "string" &&
                  typeof tab.url === "string" &&
                  (tab.url === "" || webLink(tab.url) !== null)
                : tab.kind === "turnChanges"
                  ? typeof tab.requestId === "string" &&
                    !!tab.requestId &&
                    tab.requestId.length <= 256 &&
                    typeof tab.sessionId === "string" &&
                    !!tab.sessionId &&
                    typeof tab.title === "string" &&
                    (tab.file === undefined || typeof tab.file === "string")
                  : tab.kind === "subagent"
                    ? typeof tab.taskId === "string" &&
                      !!tab.taskId &&
                      typeof tab.title === "string"
                    : tab.kind === "terminal" &&
                      tab.terminal?.id === tab.id &&
                      typeof tab.terminal.project_id === "string" &&
                      typeof tab.terminal.title === "string";
        if (valid) ids.add(tab.id);
        return valid;
      });
      const reviewTabs = tabs.filter(
        (tab) => tab.kind === "diff" || tab.kind === "turnChanges",
      );
      const review =
        reviewTabs.find((tab) => tab.id === saved.current) ?? reviewTabs.at(-1);
      if (review && reviewTabs.some((tab) => tab.kind === "turnChanges")) {
        const unified: WorkspaceTab =
          review.kind === "turnChanges"
            ? {
                id: "diff",
                kind: "diff",
                requestId: review.requestId,
                snapshotSessionId: review.sessionId,
                file: review.file,
                hostId: review.hostId,
                project: review.project,
              }
            : { ...review, id: "diff" };
        tabs = tabs.flatMap((tab) =>
          tab === reviewTabs[0]
            ? [unified]
            : reviewTabs.includes(tab)
              ? []
              : [tab],
        );
        if (reviewTabs.some((tab) => tab.id === saved.current))
          saved.current = "diff";
      }
      state = {
        tabs,
        current: tabs.some((t) => t.id === saved.current)
          ? saved.current
          : (tabs.at(-1)?.id ?? ""),
        visible:
          saved.visible !== false &&
          !(saved.tabs.length > 0 && tabs.length === 0 && !saved.expanded),
        expanded: saved.expanded === true,
        conversationActive:
          saved.conversationActive === true ||
          (saved.expanded === true && !tabs.length),
        width:
          typeof saved.width === "number" && Number.isFinite(saved.width)
            ? Math.max(WORKSPACE_MIN_WIDTH, saved.width)
            : 350,
      };
    }
  } catch {
    /* Invalid or unavailable storage falls back to a new empty sidebar. */
  }
  cache.set(key, state);
  return state;
}
export function updateWorkspace(
  key: string,
  update:
    | Partial<WorkspaceState>
    | ((state: WorkspaceState) => Partial<WorkspaceState>),
) {
  const previous = readWorkspace(key);
  const next = {
    ...previous,
    ...(typeof update === "function" ? update(previous) : update),
  };
  if (!next.tabs.some((tab) => tab.id === next.current))
    next.current = next.tabs.at(-1)?.id ?? "";
  if (next.expanded && !next.tabs.length) next.conversationActive = true;
  cache.set(key, next);
  try {
    localStorage.setItem(prefix + key, JSON.stringify(next));
  } catch {
    /* Keep in-memory state if storage is unavailable. */
  }
  listeners.forEach((listener) => listener());
}
export function copyWorkspace(from: string, to: string) {
  // The first submitted message turns a draft into a persisted conversation.
  updateWorkspace(to, readWorkspace(from));
}
export function useWorkspaceState(key: string) {
  const state = useSyncExternalStore(
    (callback) => {
      listeners.add(callback);
      return () => {
        listeners.delete(callback);
      };
    },
    () => readWorkspace(key),
  );
  return [
    state,
    (update: Parameters<typeof updateWorkspace>[1]) =>
      updateWorkspace(key, update),
  ] as const;
}

export function openSubagents(workspaceId: string) {
  updateWorkspace(workspaceId, (state) => ({
    tabs: state.tabs.some((tab) => tab.id === "subagents")
      ? state.tabs
      : [...state.tabs, { id: "subagents", kind: "subagents" }],
    current: "subagents",
    visible: true,
    conversationActive: false,
  }));
}

export function openSubagent(workspaceId: string, task: Subagent) {
  const id = `subagent:${task.id}`;
  updateWorkspace(workspaceId, (state) => ({
    tabs: state.tabs.some((tab) => tab.id === id)
      ? state.tabs
      : [
          ...state.tabs,
          { id, kind: "subagent", taskId: task.id, title: task.title },
        ],
    current: id,
    visible: true,
    conversationActive: false,
  }));
}

export function openWorkspaceResource(
  workspaceId: string,
  kind: "files" | "diff",
  location: { path?: string; file?: string } = {},
) {
  updateWorkspace(workspaceId, (state) => ({
    tabs: state.tabs.some((tab) => tab.id === kind)
      ? state.tabs.map((tab) =>
          tab.id === kind
            ? kind === "diff"
              ? { id: kind, kind, ...location }
              : tab.kind !== "files"
                ? tab
                : {
                    ...tab,
                    reference: undefined,
                    sessionId: undefined,
                    ...location,
                  }
            : tab,
        )
      : [...state.tabs, { id: kind, kind, ...location }],
    current: kind,
    visible: true,
    conversationActive: false,
  }));
}

export function openTurnChanges(
  workspaceId: string,
  sessionId: string,
  requestId: string,
  at: number | null,
  file?: string,
) {
  updateWorkspace(workspaceId, (state) => {
    const tab: WorkspaceTab = {
      id: "diff",
      kind: "diff",
      requestId,
      snapshotSessionId: sessionId,
      snapshotAt: at,
      file,
    };
    return {
      tabs: state.tabs.some((item) => item.id === "diff")
        ? state.tabs.map((item) => (item.id === "diff" ? tab : item))
        : [...state.tabs, tab],
      current: "diff",
      visible: true,
      conversationActive: false,
    };
  });
}

export function openBrowser(workspaceId: string, value = "", newTab = false) {
  const url = value ? webLink(value) : "";
  if (url === null) return;
  updateWorkspace(workspaceId, (state) => {
    const existing =
      url && !newTab
        ? state.tabs.find((tab) => tab.kind === "browser" && tab.url === url)
        : undefined;
    if (
      !existing &&
      state.tabs.filter((tab) => tab.kind === "browser").length >= 32
    )
      throw new Error("最多打开 32 个浏览器标签");
    const id = existing?.id ?? crypto.randomUUID();
    return {
      tabs: existing
        ? state.tabs
        : [
            ...state.tabs,
            {
              id,
              kind: "browser",
              url,
              title: url ? new URL(url).hostname : "浏览器",
            },
          ],
      current: id,
      visible: true,
      conversationActive: false,
    };
  });
}

export function openTaskInfo(workspaceId: string) {
  updateWorkspace(workspaceId, (state) => ({
    tabs: state.tabs.some((tab) => tab.id === "taskInfo")
      ? state.tabs
      : [...state.tabs, { id: "taskInfo", kind: "taskInfo" }],
    current: "taskInfo",
    visible: true,
    expanded: false,
    conversationActive: false,
  }));
}

/** Reuse the file browser, retaining the exact explicitly selected reference scope. */
export function openArtifact(
  workspaceId: string,
  entry: FileEntry,
  owner: { hostId?: string; project?: Project; sessionId?: string } = {},
  selectedFile?: string,
) {
  const hostPath = (path: string) =>
    path.startsWith("/") || !owner.project
      ? path
      : `${owner.project.path.replace(/\/$/, "")}/${path.replace(/^\.\//, "")}`.replace(
          /\/\.$/,
          "",
        );
  const reference =
    entry.path.startsWith("/") || entry.directory
      ? { ...entry, path: hostPath(entry.path) }
      : undefined;
  const file =
    selectedFile !== undefined
      ? reference
        ? hostPath(selectedFile)
        : selectedFile
      : entry.directory
        ? ""
        : entry.path;
  const tab: WorkspaceTab = {
    id: "files",
    kind: "files",
    ...owner,
    reference,
    path: entry.directory
      ? reference!.path
      : file.split("/").slice(0, -1).join("/"),
    file,
  };
  updateWorkspace(workspaceId, (state) => ({
    tabs: state.tabs.some((item) => item.id === "files")
      ? state.tabs.map((item) => (item.id === "files" ? tab : item))
      : [...state.tabs, tab],
    current: "files",
    visible: true,
    conversationActive: false,
  }));
}
