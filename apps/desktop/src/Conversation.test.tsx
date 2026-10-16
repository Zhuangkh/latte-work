import { referencePrompt } from "./referenceMessage";
// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus } from "./test/renderWithStatus";
import { Conversation } from "./Conversation";
import { DraftStore } from "./drafts";
import type { Event, Session } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
vi.mock("./useAgentPreferences", () => ({
  useAgentPreferences: () => ({
    model: null,
    effort: null,
    permissionMode: null,
  }),
}));
vi.mock("./ModelPicker", async () => {
  const { useEffect } = await import("react");
  const { modelSelectionScope } = await import("./modelSelectionScope");
  return {
    ModelPicker: (props: {
      hostId: string;
      projectId?: string;
      agent: string;
      value: null;
      effort: null;
      onReadyChange: (value: { scope: string; ready: boolean }) => void;
    }) => {
      useEffect(
        () =>
          props.onReadyChange({
            scope: modelSelectionScope(
              props.hostId,
              props.projectId,
              props.agent,
              props.value,
              props.effort,
            ),
            ready: true,
          }),
        [
          props.hostId,
          props.projectId,
          props.agent,
          props.value,
          props.effort,
          props.onReadyChange,
        ],
      );
      return null;
    },
  };
});
vi.mock("./PermissionPicker", () => ({ PermissionPicker: () => null }));
vi.mock("./TaskProjectPicker", () => ({ TaskProjectPicker: () => null }));
beforeEach(() => {
  HTMLElement.prototype.scrollTo = vi.fn();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  mocks.request.mockResolvedValue({
    kind: "files",
    entries: [{ name: "note.md", path: "note.md", directory: false }],
  });
});
afterEach(cleanup);
it.each(["opening", "restoring"] as const)(
  "places %s feedback in the return-to-latest slot",
  (phase) => {
    render(
      <Conversation
        {...props()}
        session={session("s")}
        agentSessionState={phase}
      />,
    );
    const label = screen.getByText(
      phase === "opening" ? "正在打开" : "正在恢复",
    );
    expect(label.closest(".composer-context")).not.toBeNull();
    expect(label.closest(".agent-session-feedback")).toBeNull();
    expect(screen.queryByRole("button", { name: "回到最新" })).toBeNull();
  },
);
const session = (id: string) =>
  ({ id, agent: "claude", status: "ready", archived: false }) as Session;
const props = () => ({
  drafts: new DraftStore(),
  hostId: "local",
  projectId: "p",
  projectName: "Test",
  projects: [],
  hosts: [],
  settingsOpen: false,
  events: [],
  connected: true,
  available: true,
  selectTaskProject: vi.fn(),
  openProject: vi.fn(),
  send: vi.fn().mockResolvedValue(true),
  cancel: vi.fn(),
  approve: vi.fn(),
});
const input = () =>
  screen.getByRole("textbox", { name: "任务输入" }) as HTMLTextAreaElement;
const approvalEvents = (id = "a"): Event[] => [
  {
    seq: 1,
    at: 0,
    session_id: id,
    event: { kind: "user", text: "检查项目", request_id: "send" },
  },
  {
    seq: 2,
    at: 1,
    session_id: id,
    event: {
      kind: "approval",
      request_id: "approval-1",
      tool: "Bash",
      input: { command: "pwd" },
    },
  },
];
it("docks approval actions above the composer while history keeps only a record", async () => {
  const p = props();
  const waiting = { ...session("a"), status: "waiting" as const };
  const events = approvalEvents();
  const view = render(
    <Conversation {...p} session={waiting} events={events} />,
  );
  const dock = await screen.findByRole("region", { name: "待审批操作" });
  const composer = view.container.querySelector(".composer-wrap")!;
  const history = view.container.querySelector(".conversation-history")!;
  expect(composer.contains(dock)).toBe(true);
  expect(history.querySelector(".approval-card")).toBeNull();
  expect(
    within(history as HTMLElement).queryByRole("button", { name: "允许一次" }),
  ).toBeNull();
  expect(within(history as HTMLElement).getByText("等待审批")).toBeTruthy();
  fireEvent.click(within(dock).getByRole("button", { name: "允许一次" }));
  await waitFor(() =>
    expect(p.approve).toHaveBeenCalledWith("approval-1", true),
  );
  view.rerender(
    <Conversation
      {...p}
      session={{ ...waiting, status: "running" }}
      events={[
        ...events,
        {
          seq: 3,
          at: 2,
          session_id: "a",
          event: {
            kind: "approval_resolved",
            request_id: "approval-1",
            allow: true,
          },
        },
      ]}
    />,
  );
  expect(screen.queryByRole("region", { name: "待审批操作" })).toBeNull();
  expect(screen.getByText("已允许")).toBeTruthy();
});
it("shows every parallel approval, disables offline decisions, and expires requests on stop", async () => {
  const p = props();
  const waiting = { ...session("a"), status: "waiting" as const };
  const events: Event[] = [
    ...approvalEvents(),
    {
      seq: 3,
      at: 2,
      session_id: "a",
      event: {
        kind: "approval",
        request_id: "approval-2",
        tool: "Read",
        input: { file_path: "README.md" },
      },
    },
  ];
  const view = render(
    <Conversation {...p} session={waiting} events={events} connected={false} />,
  );
  const dock = await screen.findByRole("region", { name: "待审批操作" });
  expect(within(dock).getByText("2 项操作等待确认")).toBeTruthy();
  for (const button of within(dock).getAllByRole("button")) {
    expect((button as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(button);
  }
  expect(p.approve).not.toHaveBeenCalled();
  view.rerender(<Conversation {...p} session={waiting} events={events} />);
  fireEvent.click(within(dock).getAllByRole("button", { name: "拒绝" })[1]);
  await waitFor(() =>
    expect(p.approve).toHaveBeenCalledWith("approval-2", false),
  );
  view.rerender(
    <Conversation
      {...p}
      session={{ ...waiting, status: "stopped" }}
      events={[
        ...events,
        {
          seq: 4,
          at: 3,
          session_id: "a",
          event: { kind: "state", status: "stopped", message: null },
        },
      ]}
    />,
  );
  expect(screen.queryByRole("region", { name: "待审批操作" })).toBeNull();
  expect(screen.getAllByText("审批已过期")).toHaveLength(2);
});
it("conceals approvals until replay is ready and never carries them into another session", async () => {
  const p = props();
  const waiting = { ...session("a"), status: "waiting" as const };
  const events = approvalEvents();
  const view = render(
    <Conversation {...p} session={waiting} events={events} historyLoading />,
  );
  expect(screen.queryByRole("region", { name: "待审批操作" })).toBeNull();
  view.rerender(<Conversation {...p} session={waiting} events={events} />);
  await screen.findByRole("region", { name: "待审批操作" });
  view.rerender(
    <Conversation
      {...p}
      session={{ ...session("b"), status: "waiting" }}
      events={events}
    />,
  );
  expect(screen.queryByRole("region", { name: "待审批操作" })).toBeNull();
  view.rerender(
    <Conversation
      {...p}
      session={{ ...waiting, archived: true }}
      events={events}
    />,
  );
  expect(screen.queryByRole("region", { name: "待审批操作" })).toBeNull();
});
it("keeps approval submission scoped when switching sessions before an earlier response returns", async () => {
  const p = props();
  const finishes: (() => void)[] = [];
  p.approve.mockImplementation(
    () => new Promise<void>((resolve) => finishes.push(resolve)),
  );
  const view = render(
    <Conversation
      {...p}
      session={{ ...session("a"), status: "waiting" }}
      events={approvalEvents("a")}
    />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "允许一次" }));
  expect(
    (screen.getByRole("button", { name: "处理中…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  view.rerender(
    <Conversation
      {...p}
      session={{ ...session("b"), status: "waiting" }}
      events={approvalEvents("b")}
    />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "允许一次" }));
  expect(p.approve).toHaveBeenCalledTimes(2);
  await act(async () => finishes[0]());
  expect(
    (screen.getByRole("button", { name: "处理中…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await act(async () => finishes[1]());
  expect(
    (screen.getByRole("button", { name: "允许一次" }) as HTMLButtonElement)
      .disabled,
  ).toBe(false);
});
it("keeps independent text and references across remounts, never replaces a nonempty draft with a suggestion", async () => {
  const p = props();
  const view = render(<Conversation {...p} key="a" session={session("a")} />);
  fireEvent.change(input(), { target: { value: "keep this" } });
  expect(screen.queryByRole("button", { name: "梳理项目结构" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(await screen.findByRole("option", { name: "note.md" }));
  view.rerender(<Conversation {...p} key="b" session={session("b")} />);
  expect(input().value).toBe("");
  fireEvent.change(input(), { target: { value: "another" } });
  view.rerender(<Conversation {...p} key="a2" session={session("a")} />);
  expect(input().value).toBe("keep this");
  expect(screen.getByRole("button", { name: "移除引用 note.md" })).toBeTruthy();
  view.rerender(
    <Conversation {...p} key="remote" hostId="remote" session={session("a")} />,
  );
  expect(input().value).toBe("");
  expect(screen.queryByRole("button", { name: "移除引用 note.md" })).toBeNull();
});
it.each([true, false])(
  "handles first-send session creation without losing or duplicating a draft (accepted=%s)",
  async (accepted) => {
    const p = props();
    let finish!: (value: boolean) => void;
    p.send.mockImplementation(
      () =>
        new Promise<boolean>((resolve) => {
          finish = resolve;
        }),
    );
    const view = render(<Conversation {...p} />);
    fireEvent.change(input(), { target: { value: "first send" } });
    fireEvent.click(screen.getByRole("button", { name: "发送任务" }));
    view.rerender(<Conversation {...p} session={session("created")} />);
    expect(input().value).toBe("first send");
    await act(async () => finish(accepted));
    await waitFor(() =>
      expect(input().value).toBe(accepted ? "" : "first send"),
    );
    view.rerender(<Conversation {...p} key="new" />);
    expect(input().value).toBe("");
  },
);
it("a late accepted send cannot clear a new draft written after navigation", async () => {
  const p = props();
  let finish!: (value: boolean) => void;
  p.send.mockImplementation(
    () =>
      new Promise<boolean>((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<Conversation {...p} key="old" session={session("a")} />);
  fireEvent.change(input(), { target: { value: "submitted" } });
  fireEvent.click(screen.getByRole("button", { name: "发送任务" }));
  view.rerender(<Conversation {...p} key="return" session={session("a")} />);
  fireEvent.change(input(), { target: { value: "new draft" } });
  await act(async () => finish(true));
  expect(input().value).toBe("new draft");
});
it.each([true, false])(
  "reconciles a session render that arrives after a fast send response (accepted=%s)",
  async (accepted) => {
    const p = props();
    p.send.mockResolvedValue(accepted);
    const view = render(<Conversation {...p} />);
    fireEvent.change(input(), { target: { value: "fast response" } });
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "发送任务" })),
    );
    view.rerender(<Conversation {...p} session={session("created")} />);
    expect(input().value).toBe(accepted ? "" : "fast response");
  },
);

it("shows actual thinking and stop progress, restoring status when cancellation fails", async () => {
  const p = props();
  let rejectStop!: (value: boolean) => void;
  p.cancel.mockImplementation(
    () =>
      new Promise<boolean>((resolve) => {
        rejectStop = resolve;
      }),
  );
  const running = { ...session("s"), status: "running" as const };
  const events = [
    {
      seq: 1,
      at: 0,
      session_id: "s",
      event: { kind: "progress" as const, phase: "thinking" as const },
    },
  ];
  const view = render(
    <Conversation {...p} session={running} events={events} />,
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("正在思考"),
  );
  fireEvent.click(screen.getByRole("button", { name: "停止任务" }));
  expect(screen.getByRole("status").textContent).toBe("正在停止");
  await act(async () => rejectStop(false));
  expect(screen.getByRole("status").textContent).toBe("正在思考");
  view.rerender(
    <Conversation
      {...p}
      session={{ ...running, status: "completed" }}
      events={events}
    />,
  );
  expect(screen.queryByRole("status")).toBeNull();
});

const repeatedEvents = (): Event[] => [
  {
    seq: 1,
    at: 0,
    session_id: "s",
    event: {
      kind: "text",
      text: Array.from(
        { length: 12 },
        (_, i) => `[${890 + i}]: (remaining tool results)\n\n`,
      ).join(""),
    },
  },
];
const repetitionWarning = "回复内容似乎在重复，可停止任务后重试。";

it("reports repetition in the status center without hiding prose or automatically stopping", async () => {
  const p = props();
  let rejectStop!: (value: boolean) => void;
  p.cancel.mockImplementation(
    () =>
      new Promise<boolean>((resolve) => {
        rejectStop = resolve;
      }),
  );
  const running = { ...session("s"), status: "running" as const };
  const view = renderWithStatus(
    <Conversation {...p} session={running} events={repeatedEvents()} />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "状态提示（1）" }));
  const warning = await screen.findByText(repetitionWarning);
  expect(warning.closest(".status-center-popover")).not.toBeNull();
  expect(warning.closest(".conversation-history")).toBeNull();
  expect(screen.getByText("[890]: (remaining tool results)")).toBeTruthy();
  expect(screen.getByText("[901]: (remaining tool results)")).toBeTruthy();
  expect(p.cancel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "停止当前任务" }));
  expect(p.cancel).toHaveBeenCalledTimes(1);
  expect(
    (screen.getByRole("button", { name: "处理中…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await act(async () => rejectStop(false));
  expect(await screen.findByText(repetitionWarning)).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      session={{ ...running, status: "stopped" }}
      events={repeatedEvents()}
    />,
  );
  expect(screen.queryByText(repetitionWarning)).toBeNull();
});

it("clears repetition feedback on disconnect, history loading, completion and scope changes", async () => {
  const p = props();
  const running = { ...session("s"), status: "running" as const };
  const repeated = repeatedEvents();
  const view = renderWithStatus(
    <Conversation {...p} session={running} events={repeated} />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "状态提示（1）" }));
  expect(await screen.findByText(repetitionWarning)).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      session={running}
      events={repeated}
      connected={false}
    />,
  );
  expect(screen.queryByText(repetitionWarning)).toBeNull();
  expect(
    (screen.getByRole("button", { name: "停止任务" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  view.rerender(
    <Conversation {...p} session={running} events={repeated} historyLoading />,
  );
  expect(screen.queryByText(repetitionWarning)).toBeNull();
  view.rerender(<Conversation {...p} session={running} events={repeated} />);
  expect(await screen.findByText(repetitionWarning)).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      session={{ ...running, status: "completed" }}
      events={repeated}
    />,
  );
  expect(screen.queryByText(repetitionWarning)).toBeNull();
  view.rerender(
    <Conversation
      {...p}
      session={{ ...running, id: "other" }}
      events={repeated}
    />,
  );
  await waitFor(() => expect(screen.queryByText(repetitionWarning)).toBeNull());
});

it("keeps reading positions across session switches and resets only on reopening or recovery", async () => {
  const p = props();
  const positions = new Map<string, { top: number; follow: boolean }>();
  const aEvents: Event[] = [
    {
      seq: 1,
      at: 0,
      session_id: "a",
      event: { kind: "text", text: "Long history" },
    },
  ];
  const bEvents: Event[] = [{ ...aEvents[0], session_id: "b" }];
  const view = render(
    <Conversation
      {...p}
      session={session("a")}
      events={aEvents}
      readingScope="a-open-1"
      readingPositions={positions}
    />,
  );
  const el = view.container.querySelector(
    ".conversation-scroll",
  ) as HTMLDivElement;
  Object.defineProperties(el, {
    scrollHeight: { configurable: true, value: 1500 },
    clientHeight: { configurable: true, value: 400 },
  });
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  el.scrollTop = 200;
  fireEvent.scroll(el);
  expect(positions.get("a-open-1")).toEqual({ top: 200, follow: false });
  view.rerender(
    <Conversation
      {...p}
      session={session("b")}
      events={bEvents}
      readingScope="b-open-1"
      readingPositions={positions}
    />,
  );
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  expect(el.scrollTop).toBe(1500);
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      events={aEvents}
      readingScope="a-open-1"
      readingPositions={positions}
    />,
  );
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  expect(el.scrollTop).toBe(200);
  expect(screen.getByRole("button", { name: "回到最新" })).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      events={aEvents}
      readingScope="a-restored-2"
      readingPositions={positions}
    />,
  );
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  expect(el.scrollTop).toBe(1500);
  expect(screen.queryByRole("button", { name: "回到最新" })).toBeNull();
});

it("keeps connection failures in the status center, preserves drafts, and offers a concise retry after failure", () => {
  const p = props();
  const reconnect = vi.fn();
  const view = renderWithStatus(
    <Conversation
      {...p}
      connected={false}
      connecting
      reconnect={reconnect}
      session={session("a")}
    />,
  );
  expect(input().placeholder).toBe("正在准备…");
  expect(screen.queryByText("暂时无法连接")).toBeNull();
  expect(
    (screen.getByRole("button", { name: "发送任务" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  fireEvent.change(input(), { target: { value: "keep draft" } });
  expect(screen.getByText("正在准备")).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      connected={false}
      connecting={false}
      connectionError="无法确认本机后台版本"
      reconnect={reconnect}
      session={session("a")}
    />,
  );
  expect(input().value).toBe("keep draft");
  expect(screen.queryByText("无法确认本机后台版本")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  expect(screen.getByText("无法确认本机后台版本")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "重新连接" }));
  expect(reconnect).toHaveBeenCalledOnce();
  view.rerender(
    <Conversation {...p} reconnect={reconnect} session={session("a")} />,
  );
  expect(screen.queryByText("暂时无法连接")).toBeNull();
  expect(input().value).toBe("keep draft");
});

it("keeps history concealed until it is fully loaded and positioned at the bottom", async () => {
  const p = props();
  const ready = vi.fn();
  const events = [
    {
      seq: 1,
      at: 0,
      session_id: "a",
      event: { kind: "text" as const, text: "Long final reply" },
    },
  ];
  const view = render(
    <Conversation
      {...p}
      session={session("a")}
      historyLoading
      onHistoryReady={ready}
      events={events}
    />,
  );
  const el = view.container.querySelector(
    ".conversation-scroll",
  ) as HTMLDivElement;
  Object.defineProperties(el, {
    scrollHeight: { configurable: true, value: 2200 },
    clientHeight: { configurable: true, value: 400 },
  });
  expect(el.getAttribute("aria-busy")).toBe("true");
  expect(ready).not.toHaveBeenCalled();
  expect(screen.queryByText("正在加载")).toBeNull();
  expect(
    view.container.querySelector(".messages")?.getAttribute("aria-hidden"),
  ).toBe("true");
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      historyLoading={false}
      onHistoryReady={ready}
      events={events}
    />,
  );
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  expect(el.scrollTop).toBe(2200);
  await waitFor(() => expect(ready).toHaveBeenCalledOnce());
  expect(screen.queryByText("正在加载")).toBeNull();
  expect(
    view.container.querySelector(".messages")?.getAttribute("aria-hidden"),
  ).toBe("false");
});

it("shows one reconnect action for a lost connection and preserves the draft", () => {
  const p = props();
  const reconnect = vi.fn();
  const view = renderWithStatus(<Conversation {...p} session={session("a")} />);
  fireEvent.change(input(), { target: { value: "保留未发送的消息" } });
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      connected={false}
      connectionLost
      connectionError="连接已失效；请求可能已执行"
      reconnect={reconnect}
    />,
  );
  expect(screen.queryByText("连接已失效；请求可能已执行")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "状态提示（1）" }));
  expect(screen.getByText("连接已失效；请求可能已执行")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "重新连接" }));
  expect(reconnect).toHaveBeenCalledOnce();
  view.rerender(<Conversation {...p} session={session("a")} />);
  expect(input().value).toBe("保留未发送的消息");
  expect(p.send).not.toHaveBeenCalled();
});

it("keeps the reading anchor when older history is prepended without concealing the current transcript", async () => {
  const p = props();
  const positions = new Map<string, { top: number; follow: boolean }>();
  const latest: Event = {
    seq: 200,
    at: 0,
    session_id: "a",
    event: { kind: "text", text: "Latest" },
  };
  const view = render(
    <Conversation
      {...p}
      session={session("a")}
      events={[latest]}
      readingScope="a"
      readingPositions={positions}
    />,
  );
  const el = view.container.querySelector(
    ".conversation-scroll",
  ) as HTMLDivElement;
  let height = 1500;
  Object.defineProperties(el, {
    scrollHeight: { configurable: true, get: () => height },
    clientHeight: { configurable: true, value: 400 },
  });
  // Commit a current layout measurement, then scroll away from the bottom.
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      events={[latest]}
      readingScope="a"
      readingPositions={positions}
    />,
  );
  await waitFor(() => expect(el.getAttribute("aria-busy")).toBe("false"));
  el.scrollTop = 200;
  fireEvent.scroll(el);
  height = 2300;
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      events={[
        { ...latest, seq: 100, event: { kind: "notice", text: "Earlier" } },
        latest,
      ]}
      readingScope="a"
      readingPositions={positions}
    />,
  );
  expect(el.scrollTop).toBe(1000);
  expect(positions.get("a")).toEqual({ top: 1000, follow: false });
  expect(el.getAttribute("aria-busy")).toBe("false");
});

it("pauses follow when jumping to a turn and removes the old rail during a conversation switch", async () => {
  const p = props();
  const positions = new Map<string, { top: number; follow: boolean }>();
  const rows: Event[] = [
    {
      seq: 1,
      at: 0,
      session_id: "a",
      event: { kind: "user", text: "较早问题", request_id: "one" },
    },
    {
      seq: 2,
      at: 1000,
      session_id: "a",
      event: { kind: "text", text: "较早回复" },
    },
    {
      seq: 3,
      at: 2000,
      session_id: "a",
      event: { kind: "state", status: "completed", message: null },
    },
    {
      seq: 4,
      at: 3000,
      session_id: "a",
      event: { kind: "user", text: "正在处理的问题", request_id: "two" },
    },
    {
      seq: 5,
      at: 4000,
      session_id: "a",
      event: { kind: "text", text: "最新过程" },
    },
  ];
  const view = render(
    <Conversation
      {...p}
      session={session("a")}
      events={rows}
      readingScope="a"
      readingPositions={positions}
    />,
  );
  const el = view.container.querySelector<HTMLDivElement>(
    ".conversation-scroll",
  )!;
  Object.defineProperties(el, {
    scrollHeight: { configurable: true, value: 1500 },
    clientHeight: { configurable: true, value: 400 },
  });
  await waitFor(() =>
    expect(screen.getByRole("navigation", { name: "对话轮次" })).toBeTruthy(),
  );
  const first = view.container.querySelector<HTMLElement>(
    '[data-turn-key="a:1"]',
  )!;
  first.getBoundingClientRect = () => new DOMRect(0, -el.scrollTop, 500, 500);
  el.getBoundingClientRect = () => new DOMRect(0, 0, 500, 400);
  const scrollTo = vi
    .spyOn(el, "scrollTo")
    .mockImplementation((options?: ScrollToOptions | number) => {
      if (typeof options === "object") el.scrollTop = options.top ?? 0;
    });
  fireEvent.click(screen.getByRole("button", { name: "跳转到：较早问题" }));
  expect(positions.get("a")).toEqual({ top: 0, follow: false });
  expect(screen.getByRole("button", { name: "回到最新" })).toBeTruthy();
  scrollTo.mockClear();
  view.rerender(
    <Conversation
      {...p}
      session={session("a")}
      events={[
        ...rows,
        {
          seq: 6,
          at: 5000,
          session_id: "a",
          event: { kind: "text", text: "继续输出" },
        },
      ]}
      readingScope="a"
      readingPositions={positions}
    />,
  );
  expect(scrollTo).not.toHaveBeenCalled();
  fireEvent.mouseEnter(
    screen.getByRole("button", { name: "跳转到：较早问题" }),
  );
  expect(screen.getByRole("tooltip")).toBeTruthy();
  view.rerender(
    <Conversation
      {...p}
      session={session("b")}
      events={rows}
      historyLoading
      readingScope="b"
      readingPositions={positions}
    />,
  );
  expect(screen.queryByRole("navigation", { name: "对话轮次" })).toBeNull();
  expect(screen.queryByRole("tooltip")).toBeNull();
});

it("opens the subagent panel instead of sending a chat message for list-agents", async () => {
  const state = props();
  const openSubagents = vi.fn();
  render(
    <Conversation
      {...state}
      session={session("s")}
      openSubagents={openSubagents}
    />,
  );
  fireEvent.change(input(), { target: { value: "/list-agents " } });
  fireEvent.keyDown(input(), { key: "Enter" });
  expect(openSubagents).toHaveBeenCalledOnce();
  expect(state.send).not.toHaveBeenCalled();
  expect(input().value).toBe("");
});

it.each(["running", "waiting", "completed"] as const)(
  "opens local navigation during %s without sending or stopping the agent",
  (status) => {
    const state = props();
    const openSubagents = vi.fn();
    render(
      <Conversation
        {...state}
        session={{ ...session("s"), status }}
        openSubagents={openSubagents}
      />,
    );
    fireEvent.change(input(), { target: { value: "/agents " } });
    const button = screen.getByRole("button", { name: "打开子智能体" });
    expect((button as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(button);
    expect(openSubagents).toHaveBeenCalledOnce();
    expect(state.send).not.toHaveBeenCalled();
    expect(state.cancel).not.toHaveBeenCalled();
    expect(input().value).toBe("");
  },
);

it("opens the local panel even without a connected available agent", () => {
  const state = props();
  const openSubagents = vi.fn();
  render(
    <Conversation
      {...state}
      connected={false}
      available={false}
      agentSessionState="unknown"
      session={session("s")}
      openSubagents={openSubagents}
    />,
  );
  fireEvent.change(input(), { target: { value: "/list-agents " } });
  const button = screen.getByRole("button", { name: "打开子智能体" });
  expect((button as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(button);
  expect(openSubagents).toHaveBeenCalledOnce();
  expect(state.send).not.toHaveBeenCalled();
  expect(state.cancel).not.toHaveBeenCalled();
});

it("keeps a pending send disabled when polling reports running, then sends the next turn once", async () => {
  const p = props();
  let finish!: (value: boolean) => void;
  p.send.mockImplementationOnce(
    () =>
      new Promise<boolean>((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<Conversation {...p} session={session("s")} />);
  fireEvent.change(input(), { target: { value: "first" } });
  fireEvent.click(screen.getByRole("button", { name: "发送任务" }));
  view.rerender(
    <Conversation {...p} session={{ ...session("s"), status: "running" }} />,
  );
  const pending = screen.getByRole("button", {
    name: "正在发送任务",
  }) as HTMLButtonElement;
  expect(pending.disabled).toBe(true);
  fireEvent.click(pending);
  expect(p.cancel).not.toHaveBeenCalled();
  expect(p.send).toHaveBeenCalledTimes(1);
  await act(async () => finish(true));
  view.rerender(
    <Conversation {...p} session={{ ...session("s"), status: "completed" }} />,
  );
  fireEvent.change(input(), { target: { value: "second" } });
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "发送任务" })),
  );
  expect(p.send).toHaveBeenCalledTimes(2);
  expect(p.send).toHaveBeenLastCalledWith(
    "second",
    null,
    null,
    null,
    undefined,
  );
  expect(input().value).toBe("");
});
it("renders a question directly, submits once for its session and preserves panel navigation without a child summary card", async () => {
  const p = props();
  const answerQuestion = vi.fn();
  let finish!: () => void;
  answerQuestion.mockImplementation(
    () =>
      new Promise<void>((done) => {
        finish = done;
      }),
  );
  const question: Event = {
    seq: 2,
    at: 1,
    session_id: "s",
    event: {
      kind: "question",
      request_id: "question",
      tool_use_id: null,
      questions: [
        {
          id: "q0",
          text: "选择输出格式",
          header: "格式",
          multi_select: false,
          options: [
            { label: "摘要", description: "简短" },
            { label: "全文", description: "完整" },
          ],
        },
      ],
    },
  };
  const events: Event[] = [
    {
      seq: 1,
      at: 0,
      session_id: "s",
      event: { kind: "user", text: "评估项目", request_id: "send" },
    },
    question,
  ];
  const openSubagents = vi.fn();
  const view = render(
    <Conversation
      {...p}
      session={{ ...session("s"), status: "waiting" }}
      events={events}
      answerQuestion={answerQuestion}
      openSubagents={openSubagents}
    />,
  );
  expect(screen.getByText("等待你的回答")).toBeTruthy();
  expect(screen.queryByText(/是否允许/)).toBeNull();
  expect(screen.queryByRole("button", { name: "查看子智能体" })).toBeNull();
  fireEvent.click(await screen.findByRole("radio", { name: /全文/ }));
  fireEvent.click(screen.getByRole("button", { name: "提交回答" }));
  fireEvent.click(screen.getByRole("button", { name: "正在提交…" }));
  expect(answerQuestion).toHaveBeenCalledOnce();
  expect(answerQuestion).toHaveBeenCalledWith("question", [
    { question_id: "q0", option_indices: [1], text: null },
  ]);
  expect(p.approve).not.toHaveBeenCalled();
  expect(p.send).not.toHaveBeenCalled();
  view.rerender(
    <Conversation
      {...p}
      session={{ ...session("other"), status: "running" }}
      events={events}
      answerQuestion={answerQuestion}
      openSubagents={openSubagents}
    />,
  );
  await act(async () => finish());
  expect(screen.queryByRole("form", { name: "待回答问题" })).toBeNull();
});

it("can queue a follow-up while the current turn runs and keeps a separate stop action", async () => {
  const p = props();
  render(
    <Conversation
      {...p}
      session={{ ...session("a"), status: "running" }}
      events={[]}
    />,
  );
  const input = screen.getByRole("textbox", { name: "任务输入" });
  fireEvent.change(input, { target: { value: "Next task" } });
  fireEvent.click(await screen.findByRole("button", { name: "加入队列" }));
  await waitFor(() =>
    expect(p.send).toHaveBeenCalledWith(
      "Next task",
      null,
      null,
      null,
      undefined,
    ),
  );
  expect(p.cancel).not.toHaveBeenCalled();
});

it("takes a queued message into the composer, retains existing draft and references, and resubmits it as a new queued message", async () => {
  const p = props();
  const old = { name: "existing.txt", path: "existing.txt", directory: false };
  const added = {
    name: "input.pdf",
    path: "/host/input.pdf",
    directory: false,
  };
  const entry = {
    request_id: "queued",
    text: referencePrompt("Queued instruction", [added]),
    model: null,
    effort: null,
    permission_mode: null,
    revision: 0,
    created_at: 1,
  };
  let pending = [entry];
  mocks.request.mockImplementation(async (_, command) => {
    if (command.method === "message_queue")
      return {
        kind: "message_queue",
        entries: pending,
        paused: false,
        reason: null,
      };
    if (command.method === "take_queued_message") {
      pending = [];
      return { kind: "queued_message_taken", entry };
    }
    return { kind: "files", entries: [] };
  });
  const key = JSON.stringify(["local", "s"]);
  p.drafts.update(key, {
    text: "Existing draft",
    references: { scope: "local:p:claude", entries: [old] },
  });
  render(
    <Conversation {...p} session={{ ...session("s"), status: "running" }} />,
  );
  fireEvent.click(
    await screen.findByRole("button", {
      name: "排队消息更多操作：Queued instruction",
    }),
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "编辑消息" }));
  await waitFor(() =>
    expect(input().value).toBe("Existing draft\n\nQueued instruction"),
  );
  expect(document.activeElement).toBe(input());
  expect(p.drafts.get(key).references.entries.map((e) => e.path)).toEqual([
    "existing.txt",
    "/host/input.pdf",
  ]);
  expect(p.send).not.toHaveBeenCalled();
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "加入队列" })),
  );
  expect(p.send).toHaveBeenCalledWith(
    referencePrompt("Existing draft\n\nQueued instruction", [old, added]),
    null,
    null,
    null,
    undefined,
  );
});
