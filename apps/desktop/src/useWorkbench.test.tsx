// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  HOST_DISCONNECTED_EVENT,
  HostConnectionError,
} from "./connectionErrors";
import { updateWorkspace } from "./workspaceState";
import { useWorkbench } from "./useWorkbench";
import type { Request, Session } from "./protocol";
const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  connect: vi.fn(),
  loadHosts: vi.fn(),
}));
vi.mock("./api", () => ({
  native: true,
  localHost: { id: "local", name: "Local" },
  loadHosts: mocks.loadHosts,
  connect: mocks.connect,
  saveHosts: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  request: mocks.request,
  message: String,
}));
let unread: boolean;
let hasMore: boolean;
let focused: boolean;
let status: Session["status"];
const session = (): Session => ({
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
  agent_session_open: true,
  agent_session_busy: false,
});
beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers();
  vi.clearAllMocks();
  mocks.loadHosts.mockResolvedValue([{ id: "local", name: "Local" }]);
  mocks.connect.mockImplementation(async (host: { id: string }) => ({
    kind: "hello",
    server_id: `server-${host.id}`,
    agents: [],
  }));
  unread = true;
  hasMore = false;
  focused = false;
  status = "completed";
  vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
  mocks.request.mockImplementation(async (_host: string, r: Request) => {
    if (r.method === "open_agent_session" || r.method === "close_agent_session")
      return { kind: "ok" };
    if (r.method === "projects")
      return {
        kind: "projects",
        projects: [{ id: "p", name: "P", path: "/p" }],
      };
    if (r.method === "sessions")
      return { kind: "sessions", sessions: [session()] };
    if (r.method === "pinned_sessions")
      return { kind: "sessions", sessions: [] };
    if (r.method === "poll")
      return {
        kind: "events",
        session: session(),
        events: [],
        has_more: hasMore,
      };
    if (r.method === "mark_session_unread") {
      unread = r.unread;
      return { kind: "session", session: session() };
    }
    throw new Error(r.method);
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const acknowledgements = () =>
  mocks.request.mock.calls.filter(
    ([, r]) => r.method === "mark_session_unread",
  );
it.each(["local", "remote"])(
  "starts in a new task after relaunch even when a %s conversation was previously selected",
  async (targetHost) => {
    mocks.loadHosts.mockResolvedValue([
      { id: "local", name: "Local" },
      { id: "remote", name: "Remote", ssh: "fixture-host" },
    ]);
    const first = renderHook(useWorkbench);
    await act(async () => {});
    expect(first.result.current.sessionId).toBe("");
    expect(first.result.current.sessions).toHaveLength(1);
    expect(first.result.current.session).toBeUndefined();
    expect(mocks.request.mock.calls.some(([, r]) => r.method === "poll")).toBe(
      false,
    );
    await act(async () =>
      first.result.current.openSession({ ...session(), hostId: targetHost }),
    );
    expect(first.result.current.sessionId).toBe("s");
    await act(async () =>
      first.result.current.refreshHost(first.result.current.host),
    );
    expect(first.result.current.sessionId).toBe("s");
    first.unmount();
    mocks.request.mockClear();
    const reopened = renderHook(useWorkbench);
    await act(async () => {});
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(reopened.result.current.connected).toBe(true);
    expect(reopened.result.current.sessionId).toBe("");
    expect(reopened.result.current.session).toBeUndefined();
    expect(reopened.result.current.events).toEqual([]);
    expect(reopened.result.current.historyLoading).toBe(false);
    expect(reopened.result.current.sessions).toHaveLength(1);
    expect(
      mocks.request.mock.calls.some(([, r]) =>
        [
          "poll",
          "history",
          "open_agent_session",
          "create_session",
          "send",
          "mark_session_unread",
        ].includes(r.method),
      ),
    ).toBe(false);
  },
);
it("creates a new conversation on the first startup submission instead of sending to an existing one", async () => {
  const base = mocks.request.getMockImplementation()!;
  const created = { ...session(), id: "new", unread: false };
  mocks.request.mockImplementation(async (host: string, r: Request) => {
    if (r.method === "create_session")
      return { kind: "session", session: created };
    if (r.method === "send") return { kind: "ok" };
    if (r.method === "poll" && r.session_id === created.id)
      return {
        kind: "events",
        session: created,
        events: [],
        has_more: false,
      };
    return base(host, r);
  });
  const { result } = renderHook(useWorkbench);
  await act(async () => {});
  expect(result.current.sessionId).toBe("");
  await act(async () => {
    expect(await result.current.send("new task", null, null, null)).toBe(true);
  });
  expect(result.current.sessionId).toBe(created.id);
  expect(
    mocks.request.mock.calls.filter(([, r]) => r.method === "create_session"),
  ).toHaveLength(1);
  const sends = mocks.request.mock.calls.filter(([, r]) => r.method === "send");
  expect(sends).toHaveLength(1);
  expect(sends[0][1]).toMatchObject({
    session_id: created.id,
    text: "new task",
  });
  await act(async () => vi.advanceTimersByTimeAsync(5000));
  expect(result.current.sessionId).toBe(created.id);
});
it("initializes each host once and refreshes the selected host without a second connection", async () => {
  const remote = { id: "remote", name: "Remote", ssh: "fixture-host" };
  mocks.loadHosts.mockResolvedValue([{ id: "local", name: "Local" }, remote]);
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  for (const host of ["local", "remote"]) {
    expect(hook.result.current.hostConnectionStatus(host)).toBe("connected");
    expect(hook.result.current.hostConnectionVersions[host]).toBe(1);
    expect(
      mocks.connect.mock.calls.filter(([h]) => h.id === host),
    ).toHaveLength(1);
    expect(
      mocks.request.mock.calls.filter(
        ([h, r]) => h === host && r.method === "projects",
      ),
    ).toHaveLength(1);
  }
  await act(async () =>
    hook.result.current.refreshHost(hook.result.current.host),
  );
  expect(
    mocks.connect.mock.calls.filter(([h]) => h.id === "local"),
  ).toHaveLength(2);
  expect(
    mocks.request.mock.calls.filter(
      ([h, r]) => h === "local" && r.method === "projects",
    ),
  ).toHaveLength(2);
  expect(hook.result.current.connected).toBe(true);
  expect(hook.result.current.hostConnectionVersions.local).toBe(2);
  expect(hook.result.current.hostConnectionVersions.remote).toBe(1);
});
it("waits for the handshake before reading cached pinned history and reporting connection state", async () => {
  let finish: (value: unknown) => void = () => {};
  mocks.connect.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  expect(hook.result.current.hostConnectionStatus("local")).toBe("connecting");
  expect(mocks.request).not.toHaveBeenCalled();
  await act(async () =>
    finish({ kind: "hello", server_id: "server-local", agents: [] }),
  );
  expect(hook.result.current.hostConnectionStatus("local")).toBe("connected");
  expect(hook.result.current.hostErrors.local).toBe("");
  expect(
    mocks.request.mock.calls.some(([, r]) => r.method === "pinned_sessions"),
  ).toBe(true);
  expect(mocks.request.mock.calls.some(([, r]) => r.method === "send")).toBe(
    false,
  );
});
it("adding a host initializes it once and preserves other hosts' live connections", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  const remote = {
    id: "remote",
    name: "Remote",
    ssh: "fixture-host",
    server_path: null,
  };
  await act(async () => hook.result.current.saveSshHost(remote));
  for (const host of ["local", "remote"]) {
    expect(
      mocks.connect.mock.calls.filter(([h]) => h.id === host),
    ).toHaveLength(1);
    expect(
      mocks.request.mock.calls.filter(
        ([h, r]) => h === host && r.method === "projects",
      ),
    ).toHaveLength(1);
  }
  await act(async () => hook.result.current.removeSshHost(remote.id));
  await act(async () => hook.result.current.saveSshHost(remote));
  expect(
    mocks.connect.mock.calls.filter(([h]) => h.id === "remote"),
  ).toHaveLength(2);
  expect(
    mocks.connect.mock.calls.filter(([h]) => h.id === "local"),
  ).toHaveLength(1);
});
it("preserves completion unread in the background and acknowledges it when viewed", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  expect(hook.result.current.session?.unread).toBe(true);
  expect(acknowledgements()).toHaveLength(0);
  focused = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(650);
  });
  expect(acknowledgements()).toHaveLength(1);
  expect(hook.result.current.session?.unread).toBe(false);
});
it("does not acknowledge running output, and marks the completed turn read only at the end of replay", async () => {
  focused = true;
  status = "running";
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  expect(acknowledgements()).toHaveLength(0);
  status = "completed";
  hasMore = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(650);
  });
  expect(acknowledgements()).toHaveLength(0);
  hasMore = false;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  expect(acknowledgements()).toHaveLength(1);
});

it("publishes initial history only after the last replay page arrives", async () => {
  const base = mocks.request.getMockImplementation()!;
  let finishLast!: (value: unknown) => void;
  let polls = 0;
  mocks.request.mockImplementation(async (host: string, r: Request) => {
    if (r.method !== "poll") return base(host, r);
    polls++;
    if (polls === 1)
      return {
        kind: "events",
        session: session(),
        events: [
          {
            seq: 1,
            at: 0,
            session_id: "s",
            event: { kind: "text", text: "First page" },
          },
        ],
        has_more: true,
      };
    return new Promise((resolve) => {
      finishLast = resolve;
    });
  });
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  expect(hook.result.current.historyLoading).toBe(true);
  expect(hook.result.current.events).toEqual([]);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    finishLast({
      kind: "events",
      session: session(),
      events: [
        {
          seq: 2,
          at: 0,
          session_id: "s",
          event: { kind: "text", text: "Last page" },
        },
      ],
      has_more: false,
    });
  });
  expect(hook.result.current.historyLoading).toBe(false);
  expect(hook.result.current.events.map((e) => e.seq)).toEqual([1, 2]);
});

it("marks a new unsaved task disconnected after metadata transport failure and reconnects without sending", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () => {
    await hook.result.current.createSession();
  });
  expect(hook.result.current.sessionId).toBe("");
  act(() =>
    window.dispatchEvent(
      new CustomEvent(HOST_DISCONNECTED_EVENT, {
        detail: new HostConnectionError("local", "连接已失效"),
      }),
    ),
  );
  expect(hook.result.current.connected).toBe(false);
  expect(hook.result.current.connectionLost).toBe(true);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(hook.result.current.connected).toBe(true);
  expect(hook.result.current.sessionId).toBe("");
  expect(
    mocks.request.mock.calls.filter(([, r]) => r.method === "send"),
  ).toHaveLength(0);
});
it("a background host's failure does not disconnect the selected host", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  act(() =>
    window.dispatchEvent(
      new CustomEvent(HOST_DISCONNECTED_EVENT, {
        detail: new HostConnectionError("other-host", "连接已关闭"),
      }),
    ),
  );
  expect(hook.result.current.connected).toBe(true);
  expect(hook.result.current.hostErrors["other-host"]).toBe("连接已关闭");
});
it("automatically recovers a background host without reconnecting the selected host or sending a prompt", async () => {
  mocks.loadHosts.mockResolvedValue([
    { id: "local", name: "Local" },
    { id: "remote", name: "Remote", ssh: "fixture-host", server_path: null },
  ]);
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  act(() =>
    window.dispatchEvent(
      new CustomEvent(HOST_DISCONNECTED_EVENT, {
        detail: new HostConnectionError("remote", "连接已关闭"),
      }),
    ),
  );
  expect(hook.result.current.connected).toBe(true);
  expect(hook.result.current.isHostConnected("remote")).toBe(false);
  expect(hook.result.current.hostConnectionStatus("remote")).toBe("failed");
  expect(hook.result.current.hostConnectionStatus("local")).toBe("connected");
  await act(async () => vi.advanceTimersByTimeAsync(5000));
  expect(hook.result.current.isHostConnected("remote")).toBe(true);
  expect(hook.result.current.hostConnectionStatus("remote")).toBe("connected");
  expect(hook.result.current.hostConnectionVersions.remote).toBe(2);
  expect(
    mocks.connect.mock.calls.filter(([h]) => h.id === "remote"),
  ).toHaveLength(2);
  expect(
    mocks.connect.mock.calls.filter(([h]) => h.id === "local"),
  ).toHaveLength(1);
  expect(mocks.request.mock.calls.some(([, r]) => r.method === "send")).toBe(
    false,
  );
});
it("does not reuse a failed replacement and restores the connection indicator without replaying a prompt", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  mocks.connect.mockRejectedValueOnce(new Error("SSH 握手失败"));
  await act(async () =>
    hook.result.current.refreshHost(hook.result.current.host),
  );
  expect(hook.result.current.hostConnectionStatus("local")).toBe("failed");
  expect(hook.result.current.connected).toBe(false);
  expect(hook.result.current.hostConnectionVersions.local).toBe(1);
  await act(async () => vi.advanceTimersByTimeAsync(5000));
  expect(hook.result.current.hostConnectionStatus("local")).toBe("connected");
  expect(hook.result.current.connected).toBe(true);
  expect(hook.result.current.hostConnectionVersions.local).toBe(2);
  expect(mocks.connect).toHaveBeenCalledTimes(3);
  expect(mocks.request.mock.calls.some(([, r]) => r.method === "send")).toBe(
    false,
  );
});
it("retains history and an ambiguous send request ID without automatically replaying the message", async () => {
  const base = mocks.request.getMockImplementation()!;
  let failSend = true;
  mocks.request.mockImplementation(async (host: string, r: Request) => {
    if (r.method === "poll")
      return {
        ...(await base(host, r)),
        events: [
          {
            seq: 1,
            at: 1,
            session_id: "s",
            event: { kind: "text", text: "保留的历史回复" },
          },
        ],
      };
    if (r.method !== "send") return base(host, r);
    if (failSend) {
      const failure = new HostConnectionError(host, "发送响应丢失");
      window.dispatchEvent(
        new CustomEvent(HOST_DISCONNECTED_EVENT, { detail: failure }),
      );
      throw failure;
    }
    return { kind: "ok" };
  });
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  const history = hook.result.current.events;
  expect(history).toHaveLength(1);
  await act(async () => {
    expect(await hook.result.current.send("hello", null, null, null)).toBe(
      false,
    );
  });
  expect(hook.result.current.connected).toBe(false);
  expect(hook.result.current.events).toEqual(history);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  const sends = () =>
    mocks.request.mock.calls.filter(([, r]) => r.method === "send");
  expect(sends()).toHaveLength(1);
  failSend = false;
  await act(async () => {
    expect(await hook.result.current.send("hello", null, null, null)).toBe(
      true,
    );
  });
  expect(sends()).toHaveLength(2);
  expect(sends()[1][1].request_id).toBe(sends()[0][1].request_id);
});

it("opens selected Agent sessions and closes explicitly without reopening on refresh", async () => {
  const { result } = renderHook(() => useWorkbench());
  await act(async () => {});
  await act(async () =>
    result.current.openSession({ ...session(), hostId: "local" }),
  );
  expect(
    mocks.request.mock.calls.some(
      ([, r]) => r.method === "open_agent_session" && r.session_id === "s",
    ),
  ).toBe(true);
  const previousScope = result.current.readingScope;
  await act(async () => {
    await result.current.sessionAction("local", {
      method: "close_agent_session",
      session_id: "s",
    });
  });
  expect(result.current.sessionId).toBe("");
  const opens = mocks.request.mock.calls.filter(
    ([, r]) => r.method === "open_agent_session",
  ).length;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(result.current.sessionId).toBe("");
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "open_agent_session",
    ),
  ).toHaveLength(opens);
  await act(async () => {
    result.current.openSession({ ...session(), hostId: "local" });
  });
  expect(result.current.readingScope).not.toBe(previousScope);
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "open_agent_session",
    ),
  ).toHaveLength(opens + 1);
});

it("distinguishes first open, recovery, and switching to a live session", async () => {
  unread = false;
  const opened = new Set<string>();
  const rows = () =>
    ["s", "t"].map((id) => ({
      ...session(),
      id,
      agent_session_open: opened.has(id),
    }));
  const finish = new Map<string, () => void>();
  const base = mocks.request.getMockImplementation()!;
  mocks.request.mockImplementation(async (host: string, r: Request) => {
    if (r.method === "sessions") return { kind: "sessions", sessions: rows() };
    if (r.method === "poll")
      return {
        kind: "events",
        session: rows().find((s) => s.id === r.session_id),
        events: [],
        has_more: false,
      };
    if (r.method === "open_agent_session")
      return new Promise((resolve) => {
        finish.set(r.session_id, () => {
          opened.add(r.session_id);
          resolve({ kind: "ok" });
        });
      });
    return base(host, r);
  });
  const { result } = renderHook(useWorkbench);
  const state = (id: string) =>
    result.current.agentSessionState({
      ...rows().find((s) => s.id === id)!,
      hostId: "local",
    });
  await act(async () => {});
  await act(async () =>
    result.current.openSession({ ...rows()[0], hostId: "local" }),
  );
  expect(state("s")).toBe("opening");
  await act(async () => finish.get("s")!());
  expect(state("s")).toBe("open");
  opened.clear();
  act(() =>
    window.dispatchEvent(
      new CustomEvent(HOST_DISCONNECTED_EVENT, {
        detail: new HostConnectionError("local", "连接已失效"),
      }),
    ),
  );
  expect(state("s")).toBe("unknown");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(state("s")).toBe("restoring");
  await act(async () => finish.get("s")!());
  expect(state("s")).toBe("open");
  await act(async () =>
    result.current.openSession({ ...rows()[1], hostId: "local" }),
  );
  expect(state("t")).toBe("opening");
  await act(async () => finish.get("t")!());
  expect(state("t")).toBe("open");
  await act(async () =>
    result.current.openSession({ ...rows()[0], hostId: "local" }),
  );
  expect(state("s")).toBe("open");
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "open_agent_session" && r.session_id === "s",
    ),
  ).toHaveLength(2);
  expect(mocks.request.mock.calls.some(([, r]) => r.method === "send")).toBe(
    false,
  );
});

it.each(["local", "remote"])(
  "returns to a live %s conversation without reconnecting, reopening or replaying history",
  async (targetHost) => {
    unread = false;
    mocks.loadHosts.mockResolvedValue([
      { id: "local", name: "Local" },
      { id: "remote", name: "Devbox", ssh: "devbox" },
    ]);
    const rows = (host: string) => [
      { ...session(), id: host === "remote" ? "remote-chat" : "s" },
    ];
    const base = mocks.request.getMockImplementation()!;
    let delayPoll = false;
    let finish!: (value: unknown) => void;
    mocks.request.mockImplementation(async (host: string, r: Request) => {
      if (r.method === "sessions")
        return { kind: "sessions", sessions: rows(host) };
      if (r.method === "poll") {
        if (host === targetHost && delayPoll && r.after === 1)
          return new Promise((resolve) => {
            finish = resolve;
          });
        return {
          kind: "events",
          session: rows(host)[0],
          has_more: false,
          events:
            r.after === 0
              ? [
                  {
                    seq: 1,
                    at: 1,
                    session_id: r.session_id,
                    event: { kind: "text", text: `${host} history` },
                  },
                ]
              : [],
        };
      }
      return base(host, r);
    });
    const { result } = renderHook(useWorkbench);
    await act(async () => {});
    const select = async (host: string) =>
      act(async () =>
        result.current.openSession({ ...rows(host)[0], hostId: host }),
      );
    await select("remote");
    expect(result.current.events[0].event).toEqual({
      kind: "text",
      text: "remote history",
    });
    await select("local");
    if (targetHost === "local") await select("remote");
    const connections = mocks.connect.mock.calls.length;
    const projects = mocks.request.mock.calls.filter(
      ([, r]) => r.method === "projects",
    ).length;
    const opens = mocks.request.mock.calls.filter(
      ([, r]) => r.method === "open_agent_session",
    ).length;
    const polls = mocks.request.mock.calls.filter(
      ([h, r]) => h === targetHost && r.method === "poll",
    ).length;
    delayPoll = true;
    await select(targetHost);
    expect(result.current.connected).toBe(true);
    expect(result.current.connecting).toBe(false);
    expect(result.current.historyLoading).toBe(false);
    expect(result.current.events[0].event).toEqual({
      kind: "text",
      text: `${targetHost} history`,
    });
    expect(mocks.connect).toHaveBeenCalledTimes(connections);
    expect(
      mocks.request.mock.calls.filter(([, r]) => r.method === "projects"),
    ).toHaveLength(projects);
    expect(
      mocks.request.mock.calls.filter(
        ([, r]) => r.method === "open_agent_session",
      ),
    ).toHaveLength(opens);
    expect(
      mocks.request.mock.calls
        .filter(([h, r]) => h === targetHost && r.method === "poll")
        .slice(polls),
    ).toEqual([
      [
        targetHost,
        { method: "poll", session_id: rows(targetHost)[0].id, after: 1 },
      ],
    ]);
    await act(async () =>
      finish({
        kind: "events",
        session: rows(targetHost)[0],
        events: [],
        has_more: false,
      }),
    );
    await act(async () => result.current.setRetry((value) => value + 1));
    expect(mocks.connect.mock.calls.length).toBeGreaterThan(connections);
    expect(mocks.request.mock.calls.some(([, r]) => r.method === "send")).toBe(
      false,
    );
  },
);

it("lets the workspace grow to the available window and preserves its preferred width through layout changes", async () => {
  vi.stubGlobal("innerWidth", 1920);
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  act(() => hook.result.current.setPanel(true));
  const dragRight = () => {
    act(() => {
      hook.result.current.resize(
        {
          preventDefault: vi.fn(),
          clientX: window.innerWidth - hook.result.current.rightWidth - 1,
          pointerId: 1,
          currentTarget: { setPointerCapture: vi.fn(), dataset: {} },
        } as unknown as React.PointerEvent<HTMLElement>,
        "right",
      );
      const sidebar = hook.result.current.sidebarOpen
        ? hook.result.current.leftWidth
        : 0;
      window.dispatchEvent(
        new MouseEvent("pointermove", {
          clientX: sidebar + 240 + (sidebar ? 1 : 0),
        }),
      );
      window.dispatchEvent(new MouseEvent("pointerup"));
    });
  };
  const resizeWindow = (width: number) => {
    act(() => {
      vi.stubGlobal("innerWidth", width);
      window.dispatchEvent(new globalThis.Event("resize"));
    });
  };
  dragRight();
  const wide = hook.result.current.rightWidth;
  expect(wide).toBeGreaterThan(600);
  expect(wide + hook.result.current.leftWidth + 2).toBe(1920 - 240);
  resizeWindow(960);
  expect(hook.result.current.rightWidth).toBe(
    960 - hook.result.current.leftWidth - 240 - 2,
  );
  resizeWindow(1920);
  expect(hook.result.current.rightWidth).toBe(wide);
  act(() => hook.result.current.toggleSidebar());
  dragRight();
  const withoutSidebar = hook.result.current.rightWidth;
  expect(withoutSidebar).toBe(1920 - 240 - 1);
  act(() => hook.result.current.toggleSidebar());
  expect(hook.result.current.rightWidth).toBe(wide);
  act(() => hook.result.current.toggleSidebar());
  expect(hook.result.current.rightWidth).toBe(withoutSidebar);
  expect(document.body.style.userSelect).toBe("");
});

it("merges once per drag, leaves navigation resizing independent, and restores a narrow conversation from the button", async () => {
  vi.stubGlobal("innerWidth", 1440);
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  act(() => hook.result.current.setPanel(true));
  const handle = document.createElement("div");
  handle.setPointerCapture = vi.fn();
  const startDrag = (side: "left" | "right", start: number) =>
    hook.result.current.resize(
      {
        preventDefault: vi.fn(),
        clientX: start,
        pointerId: 1,
        currentTarget: handle,
      } as unknown as React.PointerEvent<HTMLElement>,
      side,
    );
  const move = (x: number) =>
    window.dispatchEvent(new MouseEvent("pointermove", { clientX: x }));
  const stop = () => window.dispatchEvent(new MouseEvent("pointerup"));
  const sidebar = hook.result.current.leftWidth;
  const preferred = hook.result.current.workspace.width;
  act(() => {
    startDrag("right", 1440 - hook.result.current.rightWidth - 1);
    expect(handle.dataset.resizing).toBe("true");
    move(sidebar + 8);
    move(sidebar + 400);
    stop();
  });
  expect(handle.dataset.resizing).toBeUndefined();
  expect(hook.result.current.workspace).toMatchObject({
    expanded: true,
    conversationActive: true,
    width: preferred,
  });
  act(() => hook.result.current.setSessionId("other"));
  expect(hook.result.current.workspace.expanded).toBe(false);
  act(() => hook.result.current.setSessionId("s"));
  expect(hook.result.current.workspace).toMatchObject({
    expanded: true,
    conversationActive: true,
  });
  act(() => {
    startDrag("left", sidebar);
    move(sidebar + 100);
    stop();
  });
  expect(hook.result.current.leftWidth).toBe(320);
  expect(hook.result.current.workspace).toMatchObject({
    expanded: true,
    width: preferred,
  });
  act(() => {
    startDrag("left", 320);
    move(100);
    stop();
  });
  expect(hook.result.current.leftWidth).toBe(200);
  expect(hook.result.current.workspace.expanded).toBe(true);
  act(() => {
    startDrag("right", 200);
    move(600);
    stop();
  });
  expect(hook.result.current.workspace.expanded).toBe(true);
  act(() => hook.result.current.restoreWorkspace());
  expect(hook.result.current.workspace).toMatchObject({
    visible: true,
    expanded: false,
    width: 1440 - 200 - 320 - 2,
  });
  expect(document.body.style.userSelect).toBe("");
});
it("restores a split without left navigation and cleans the drag highlight on cancellation", async () => {
  vi.stubGlobal("innerWidth", 960);
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  act(() => {
    hook.result.current.setPanel(true);
    hook.result.current.toggleSidebar();
  });
  act(() => hook.result.current.restoreWorkspace());
  expect(hook.result.current.rightWidth).toBe(960 - 320 - 1);
  const handle = document.createElement("div");
  handle.setPointerCapture = vi.fn();
  act(() =>
    hook.result.current.resize(
      {
        preventDefault: vi.fn(),
        clientX: 320,
        pointerId: 1,
        currentTarget: handle,
      } as unknown as React.PointerEvent<HTMLElement>,
      "right",
    ),
  );
  expect(handle.dataset.resizing).toBe("true");
  act(() => window.dispatchEvent(new MouseEvent("pointercancel")));
  expect(handle.dataset.resizing).toBeUndefined();
  expect(document.body.style.userSelect).toBe("");
});

it.each(["local", "remote"])(
  "reveals a recent %s window without replaying old pages and retains its raw cursor across live polling and navigation",
  async (targetHost) => {
    unread = false;
    mocks.loadHosts.mockResolvedValue([
      { id: "local", name: "Local" },
      { id: "remote", name: "Remote", ssh: "fixture" },
    ]);
    mocks.connect.mockImplementation(async (host: { id: string }) => ({
      kind: "hello",
      server_id: `server-${host.id}`,
      agents: [],
      history_window: true,
    }));
    const base = mocks.request.getMockImplementation()!;
    mocks.request.mockImplementation(async (host: string, r: Request) => {
      if (r.method === "history")
        return {
          kind: "history",
          session: session(),
          has_more: r.before === null,
          needs_earlier: false,
          before: r.before === null ? 3000 : null,
          events: [
            {
              seq: r.before === null ? 4000 : 2000,
              at: 1,
              session_id: "s",
              event: {
                kind: "text",
                text:
                  r.before === null ? "Complete latest reply" : "Earlier reply",
              },
            },
          ],
        };
      if (r.method === "poll")
        return {
          kind: "events",
          session: session(),
          has_more: false,
          events:
            r.after === 4000
              ? [
                  {
                    seq: 4001,
                    at: 2,
                    session_id: "s",
                    event: { kind: "notice", text: "Live update" },
                  },
                ]
              : [],
        };
      return base(host, r);
    });
    const { result } = renderHook(useWorkbench);
    await act(async () => {});
    await act(async () =>
      result.current.openSession({ ...session(), hostId: targetHost }),
    );
    expect(result.current.historyLoading).toBe(false);
    expect(result.current.hasEarlierHistory).toBe(true);
    expect(result.current.events[0].seq).toBe(4000);
    expect(
      mocks.request.mock.calls.filter(
        ([h, r]) =>
          h === targetHost && r.method === "history" && r.before === null,
      ),
    ).toHaveLength(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(650);
    });
    expect(result.current.events.at(-1)?.seq).toBe(4001);
    await act(async () => result.current.loadEarlierHistory());
    expect(mocks.request).toHaveBeenCalledWith(targetHost, {
      method: "history",
      session_id: "s",
      before: 3000,
    });
    expect(result.current.events.map((e) => e.seq)).toEqual([2000, 4000, 4001]);
    expect(result.current.hasEarlierHistory).toBe(false);
    await act(async () => result.current.createSession());
    const reads = mocks.request.mock.calls.filter(
      ([h, r]) => h === targetHost && r.method === "history",
    ).length;
    await act(async () =>
      result.current.openSession({ ...session(), hostId: targetHost }),
    );
    expect(result.current.historyLoading).toBe(false);
    expect(result.current.events.map((e) => e.seq)).toEqual([2000, 4000, 4001]);
    expect(
      mocks.request.mock.calls.filter(
        ([h, r]) => h === targetHost && r.method === "history",
      ),
    ).toHaveLength(reads);
  },
);

it("assembles a split final reply before revealing history without reading unrelated old pages", async () => {
  mocks.connect.mockResolvedValue({
    kind: "hello",
    server_id: "server-local",
    agents: [],
    history_window: true,
  });
  const base = mocks.request.getMockImplementation()!;
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementation(async (host: string, r: Request) => {
    if (r.method !== "history") return base(host, r);
    if (r.before !== null)
      return new Promise((resolve) => {
        finish = resolve;
      });
    return {
      kind: "history",
      session: session(),
      has_more: true,
      needs_earlier: true,
      before: 100,
      events: [
        {
          seq: 200,
          at: 1,
          session_id: "s",
          event: { kind: "text", text: "end" },
        },
      ],
    };
  });
  const { result } = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    result.current.openSession({ ...session(), hostId: "local" }),
  );
  expect(result.current.historyLoading).toBe(true);
  expect(result.current.events).toEqual([]);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () =>
    finish({
      kind: "history",
      session: session(),
      has_more: true,
      needs_earlier: false,
      before: 50,
      events: [
        {
          seq: 99,
          at: 0,
          session_id: "s",
          event: { kind: "text", text: "begin" },
        },
      ],
    }),
  );
  expect(result.current.historyLoading).toBe(false);
  expect(result.current.events.map((e) => e.seq)).toEqual([99, 200]);
  expect(result.current.hasEarlierHistory).toBe(true);
  expect(
    mocks.request.mock.calls.filter(([, r]) => r.method === "history"),
  ).toHaveLength(2);
});

it.each([
  "/agents inspect",
  "/list-agents all",
  "/tasks 参数",
  "/agents\n请继续检查",
])(
  "opens local navigation for %s without admitting a turn or discarding arguments",
  async (text) => {
    const hook = renderHook(useWorkbench);
    await act(async () => {});
    const before = hook.result.current.workspaceId;
    mocks.request.mockClear();
    let accepted = true;
    await act(async () => {
      accepted = await hook.result.current.send(
        text,
        null,
        null,
        null,
        "subagents",
      );
    });
    expect(accepted).toBe(false);
    expect(hook.result.current.sessionId).toBe("");
    expect(hook.result.current.workspaceId).toBe(before);
    expect(hook.result.current.workspace).toMatchObject({
      visible: true,
      current: "subagents",
    });
    expect(hook.result.current.error).toContain("此命令不接受参数");
    expect(mocks.request).not.toHaveBeenCalled();
  },
);

it("treats adapter-provided UI hints as local navigation before any session creation", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  mocks.request.mockClear();
  await act(async () => {
    expect(
      await hook.result.current.send(
        "/native-list",
        null,
        null,
        null,
        "subagents",
      ),
    ).toBe(true);
  });
  expect(hook.result.current.sessionId).toBe("");
  expect(hook.result.current.workspace.current).toBe("subagents");
  expect(mocks.request).not.toHaveBeenCalled();
});

it.each(["/list-agents", "/agents", "/tasks", "/subagents"])(
  "opens %s without creating a conversation or sending an agent request",
  async (command) => {
    const hook = renderHook(useWorkbench);
    await act(async () => {});
    const workspaceId = hook.result.current.workspaceId;
    mocks.request.mockClear();
    let accepted = false;
    await act(async () => {
      accepted = await hook.result.current.send(command, null, null, null);
    });
    expect(accepted).toBe(true);
    expect(hook.result.current.sessionId).toBe("");
    expect(hook.result.current.workspaceId).toBe(workspaceId);
    expect(hook.result.current.workspace).toMatchObject({
      visible: true,
      current: "subagents",
      conversationActive: false,
    });
    expect(mocks.request).not.toHaveBeenCalled();
  },
);

it("keeps the selected conversation and its status unchanged when opening the local agent list", async () => {
  const hook = renderHook(useWorkbench);
  await act(async () => {});
  await act(async () =>
    hook.result.current.openSession({ ...session(), hostId: "local" }),
  );
  const before = hook.result.current.session;
  mocks.request.mockClear();
  await act(async () =>
    hook.result.current.send("  /list-agents ", null, null, null),
  );
  expect(hook.result.current.session).toEqual(before);
  expect(hook.result.current.workspace.current).toBe("subagents");
  expect(mocks.request).not.toHaveBeenCalled();
});

it.each(["/model", "/effort high"])(
  "guards %s before session persistence even if a caller bypasses the composer",
  async (text) => {
    const hook = renderHook(useWorkbench);
    await act(async () => {});
    mocks.request.mockClear();
    await act(async () => {
      expect(await hook.result.current.send(text, null, null, null)).toBe(
        false,
      );
    });
    expect(hook.result.current.sessionId).toBe("");
    expect(hook.result.current.error).toContain("模型菜单");
    expect(mocks.request).not.toHaveBeenCalled();
  },
);
