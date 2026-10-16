// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import {
  copyWorkspace,
  readWorkspace,
  updateWorkspace,
  workspaceKey,
  openSubagent,
} from "./workspaceState";
beforeEach(() => localStorage.clear());
it("drops saved source tabs and selects a surviving file tab on restart", async () => {
  const key = crypto.randomUUID();
  localStorage.setItem(
    "latte-work.conversation-workspace.v1:" + key,
    JSON.stringify({
      tabs: [
        { id: "sources", kind: "sources" },
        { id: "files", kind: "files", path: "src", file: "src/main.ts" },
        {
          id: "source:/outside/file.txt",
          kind: "source",
          source: { path: "/outside/file.txt", name: "file.txt" },
        },
      ],
      current: "source:/outside/file.txt",
      visible: true,
      expanded: false,
      width: 420,
    }),
  );
  vi.resetModules();
  const reloaded = await import("./workspaceState");
  expect(reloaded.readWorkspace(key)).toMatchObject({
    tabs: [{ id: "files", kind: "files", path: "src", file: "src/main.ts" }],
    current: "files",
    visible: true,
    width: 420,
  });
});
it.each([false, true])(
  "removes source-only tabs and restores conversation in expanded=%s layout",
  (expanded) => {
    const key = crypto.randomUUID();
    localStorage.setItem(
      "latte-work.conversation-workspace.v1:" + key,
      JSON.stringify({
        tabs: [{ id: "sources", kind: "sources" }],
        current: "sources",
        visible: true,
        expanded,
        conversationActive: false,
      }),
    );
    expect(readWorkspace(key)).toMatchObject({
      tabs: [],
      current: "",
      visible: expanded,
      expanded,
      conversationActive: expanded,
    });
  },
);
it("restores named child tabs with stable IDs without sharing another conversation", async () => {
  const key = crypto.randomUUID();
  const task = {
    id: "child",
    native_id: "native",
    tool_use_id: null,
    title: "Child task",
    status: "completed" as const,
    summary: null,
    last_tool: null,
    started_at: 1,
    updated_at: 2,
  };
  openSubagent(key, task);
  openSubagent(key, task);
  vi.resetModules();
  const reloaded = await import("./workspaceState");
  expect(reloaded.readWorkspace(key).tabs).toEqual([
    {
      id: "subagent:child",
      kind: "subagent",
      taskId: "child",
      title: "Child task",
    },
  ]);
  expect(reloaded.readWorkspace(crypto.randomUUID()).tabs).toEqual([]);
});
it("persists layout, selection and file navigation across a fresh module load", async () => {
  const key = workspaceKey(crypto.randomUUID());
  updateWorkspace(key, {
    tabs: [{ id: "files", kind: "files", path: "src", file: "src/main.ts" }],
    current: "files",
    visible: false,
    expanded: true,
    conversationActive: true,
    width: 1180,
  });
  vi.resetModules();
  const reloaded = await import("./workspaceState");
  expect(reloaded.readWorkspace(key)).toEqual(readWorkspace(key));
  expect(
    reloaded.readWorkspace(workspaceKey("different-conversation")).tabs,
  ).toEqual([]);
});
it("migrates draft state on first send while new drafts start empty", () => {
  const draft = workspaceKey(crypto.randomUUID());
  const session = workspaceKey(crypto.randomUUID());
  updateWorkspace(draft, {
    tabs: [{ id: "diff", kind: "diff" }],
    current: "diff",
    visible: false,
    width: 520,
  });
  copyWorkspace(draft, session);
  expect(readWorkspace(session)).toEqual(readWorkspace(draft));
  updateWorkspace(session, { tabs: [] });
  expect(readWorkspace(draft).tabs).toHaveLength(1);
  expect(readWorkspace(workspaceKey(crypto.randomUUID())).tabs).toEqual([]);
});
it("keeps each conversation's visibility and width independent", () => {
  const a = workspaceKey(crypto.randomUUID()),
    b = workspaceKey(crypto.randomUUID());
  updateWorkspace(a, { visible: false, width: 550, expanded: true });
  updateWorkspace(b, { visible: true, width: 300 });
  expect(readWorkspace(a)).toMatchObject({
    visible: false,
    width: 550,
    expanded: true,
  });
  expect(readWorkspace(b)).toMatchObject({
    visible: true,
    width: 300,
    expanded: false,
  });
});

it("preserves a conversation saved by the earlier preview without making host/project part of its new key", () => {
  const id = crypto.randomUUID();
  localStorage.setItem(
    "latte-work.conversation-workspace.v1:" +
      JSON.stringify(["host", "project", id]),
    JSON.stringify({
      tabs: [{ id: "files", kind: "files" }],
      current: "files",
      visible: false,
      width: 420,
    }),
  );
  expect(readWorkspace(id)).toMatchObject({
    current: "files",
    visible: false,
    width: 420,
  });
  expect(
    localStorage.getItem("latte-work.conversation-workspace.v1:" + id),
  ).not.toBeNull();
});

it("keeps a new task focused on conversation while restoring explicit panel preferences", async () => {
  const fresh = workspaceKey(crypto.randomUUID());
  expect(readWorkspace(fresh)).toMatchObject({ visible: false, tabs: [] });
  updateWorkspace(fresh, { visible: true });
  vi.resetModules();
  const reloaded = await import("./workspaceState");
  expect(reloaded.readWorkspace(fresh).visible).toBe(true);
});

it("selects conversation when the last resource closes in merged mode", () => {
  const key = crypto.randomUUID();
  updateWorkspace(key, {
    expanded: true,
    tabs: [{ id: "files", kind: "files" }],
    current: "files",
    conversationActive: false,
  });
  updateWorkspace(key, { tabs: [] });
  expect(readWorkspace(key)).toMatchObject({
    expanded: true,
    conversationActive: true,
    current: "",
  });
});
it("reuses a single review tab for historical turns and the overview, and migrates old duplicate tabs", async () => {
  const { openTurnChanges, openWorkspaceResource, readWorkspace } =
    await import("./workspaceState");
  const key = crypto.randomUUID();
  openTurnChanges(key, "session", "r1", 1, "one.ts");
  openTurnChanges(key, "session", "r2", 2, "two.ts");
  expect(readWorkspace(key).tabs).toEqual([
    {
      id: "diff",
      kind: "diff",
      requestId: "r2",
      snapshotSessionId: "session",
      snapshotAt: 2,
      file: "two.ts",
    },
  ]);
  openWorkspaceResource(key, "diff");
  expect(readWorkspace(key).tabs).toEqual([{ id: "diff", kind: "diff" }]);
  const restored = crypto.randomUUID();
  localStorage.setItem(
    "latte-work.conversation-workspace.v1:" + restored,
    JSON.stringify({
      tabs: [
        { id: "diff", kind: "diff" },
        {
          id: "turnChanges:r1",
          kind: "turnChanges",
          requestId: "r1",
          sessionId: "session",
          title: "Old",
        },
        {
          id: "turnChanges:r2",
          kind: "turnChanges",
          requestId: "r2",
          sessionId: "session",
          title: "Selected",
          file: "two.ts",
        },
      ],
      current: "turnChanges:r2",
    }),
  );
  expect(readWorkspace(restored)).toMatchObject({
    current: "diff",
    tabs: [
      {
        id: "diff",
        kind: "diff",
        requestId: "r2",
        snapshotSessionId: "session",
        file: "two.ts",
      },
    ],
  });
});

it("pins task information in the right pane, reuses it, and restores it independently per conversation", async () => {
  const state = await import("./workspaceState");
  const key = crypto.randomUUID();
  state.updateWorkspace(key, { expanded: true, conversationActive: true });
  state.openTaskInfo(key);
  state.openTaskInfo(key);
  expect(state.readWorkspace(key)).toMatchObject({
    tabs: [{ id: "taskInfo", kind: "taskInfo" }],
    current: "taskInfo",
    expanded: false,
    conversationActive: false,
    visible: true,
  });
  vi.resetModules();
  const fresh = await import("./workspaceState");
  expect(fresh.readWorkspace(key).tabs).toEqual([
    { id: "taskInfo", kind: "taskInfo" },
  ]);
  expect(fresh.readWorkspace(crypto.randomUUID()).tabs).toEqual([]);
});

it("uses the shared file tab and consistent host paths for project-local Skill trees", async () => {
  const {
    openArtifact,
    openTaskInfo,
    readWorkspace: read,
  } = await import("./workspaceState");
  const key = crypto.randomUUID();
  const owner = {
    hostId: "ssh",
    project: { id: "p", name: "P", path: "/project" },
    sessionId: "s",
  };
  openTaskInfo(key);
  openArtifact(
    key,
    { name: "review", path: ".claude/skills/review", directory: true },
    owner,
    ".claude/skills/review/SKILL.md",
  );
  expect(read(key)).toMatchObject({
    current: "files",
    tabs: [
      { kind: "taskInfo" },
      {
        kind: "files",
        file: "/project/.claude/skills/review/SKILL.md",
        reference: { path: "/project/.claude/skills/review", directory: true },
      },
    ],
  });
  openArtifact(
    key,
    { name: "report.md", path: "report.md", directory: false },
    owner,
  );
  expect(read(key).tabs).toHaveLength(2);
  expect(read(key).tabs[1]).toMatchObject({
    kind: "files",
    file: "report.md",
    reference: undefined,
  });
});
