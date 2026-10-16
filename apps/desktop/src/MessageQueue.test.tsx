// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { MessageQueue } from "./MessageQueue";
import { referencePrompt } from "./referenceMessage";
import type { QueuedMessage } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const props = {
  hostId: "ssh",
  sessionId: "s",
  active: true,
  connected: true,
  revision: "0",
  running: true,
};
let entry: QueuedMessage;
beforeEach(() => {
  vi.stubGlobal("PointerEvent", MouseEvent);
  entry = {
    request_id: "q",
    text: referencePrompt("Original message", [
      { name: "input.pdf", path: "/host/input.pdf", directory: false },
    ]),
    model: null,
    effort: null,
    permission_mode: null,
    revision: 3,
    created_at: 1,
  };
  mocks.request.mockReset().mockResolvedValue({
    kind: "message_queue",
    entries: [entry],
    paused: true,
    reason: "后台已重启",
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
it("takes an edited message out of the queue before restoring it to the composer", async () => {
  const onEdit = vi.fn();
  render(<MessageQueue {...props} onEdit={onEdit} />);
  fireEvent.click(
    await screen.findByRole("button", {
      name: "排队消息更多操作：Original message",
    }),
  );
  mocks.request.mockImplementation(async (_, command) =>
    command.method === "take_queued_message"
      ? { kind: "queued_message_taken", entry }
      : { kind: "message_queue", entries: [], paused: true, reason: null },
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "编辑消息" }));
  await waitFor(() => expect(onEdit).toHaveBeenCalledWith(entry));
  expect(mocks.request).toHaveBeenCalledWith("ssh", {
    method: "take_queued_message",
    session_id: "s",
    request_id: "q",
    revision: 3,
  });
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(
    mocks.request.mock.calls.every(
      ([, r]) =>
        !["send", "send_queued", "edit_queued_message"].includes(r.method),
    ),
  ).toBe(true);
});
it("offers an explicit idempotent edit recovery after an ambiguous take response", async () => {
  const onEdit = vi.fn();
  render(<MessageQueue {...props} onEdit={onEdit} />);
  fireEvent.click(
    await screen.findByRole("button", {
      name: "排队消息更多操作：Original message",
    }),
  );
  mocks.request.mockImplementation(async (_, command) => {
    if (command.method === "take_queued_message") throw new Error("响应丢失");
    return { kind: "message_queue", entries: [], paused: true, reason: null };
  });
  fireEvent.click(screen.getByRole("menuitem", { name: "编辑消息" }));
  await waitFor(() =>
    expect(screen.queryByText("Original message")).toBeNull(),
  );
  expect(onEdit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: /状态提示/ }));
  const recover = await screen.findByRole("button", { name: "恢复编辑内容" });
  mocks.request.mockResolvedValueOnce({ kind: "queued_message_taken", entry });
  fireEvent.click(recover);
  await waitFor(() => expect(onEdit).toHaveBeenCalledTimes(1));
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "take_queued_message",
    ),
  ).toHaveLength(2);
});
it("keeps the row after an ambiguous removal and requires explicit refresh", async () => {
  render(<MessageQueue {...props} />);
  const button = await screen.findByRole("button", {
    name: "删除排队消息：Original message",
  });
  mocks.request.mockRejectedValueOnce(new Error("响应丢失，请核对队列"));
  fireEvent.click(button);
  fireEvent.click(await screen.findByRole("button", { name: /状态提示/ }));
  expect(await screen.findByText("Error: 响应丢失，请核对队列")).toBeTruthy();
  expect(screen.getByText("Original message")).toBeTruthy();
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "cancel_queued_message",
    ),
  ).toHaveLength(1);
});
it("resumes only after the user clicks and never re-sends the queued text", async () => {
  render(<MessageQueue {...props} />);
  await screen.findByText("后台已重启");
  expect(
    mocks.request.mock.calls.every(([, r]) => r.method === "message_queue"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "继续排队" }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "resume_message_queue",
      session_id: "s",
      provider: null,
    }),
  );
});
it("stops polling hidden panes and fences a late response after changing hosts", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<MessageQueue {...props} />);
  view.rerender(<MessageQueue {...props} hostId="other" connected={false} />);
  await act(async () =>
    finish({
      kind: "message_queue",
      entries: [entry],
      paused: false,
      reason: null,
    }),
  );
  expect(screen.queryByRole("region", { name: "待发送消息" })).toBeNull();
  expect(mocks.request).toHaveBeenCalledTimes(1);
});

function multiple() {
  return [
    entry,
    { ...entry, request_id: "b", text: "Second" },
    { ...entry, request_id: "c", text: "Third" },
  ];
}
it("persists drag ordering through the server with a full expected snapshot", async () => {
  const entries = multiple();
  let pending = entries;
  mocks.request.mockImplementation(async (_, request) => {
    if (request.method === "reorder_message_queue")
      pending = request.order.map((id: string) =>
        entries.find((item) => item.request_id === id)!,
      );
    return {
      kind: "message_queue",
      entries: pending,
      paused: false,
      reason: null,
    };
  });
  render(<MessageQueue {...props} />);
  const grip = await screen.findByRole("button", {
    name: "排序排队消息：Third",
  });
  const rows = screen.getAllByRole("listitem");
  vi.spyOn(screen.getByRole("list"), "getBoundingClientRect").mockReturnValue({
    left: 0,
    right: 300,
    top: 0,
    bottom: 120,
  } as DOMRect);
  rows.forEach((row, index) =>
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue({
      top: index * 40,
      bottom: (index + 1) * 40,
      height: 40,
    } as DOMRect),
  );
  fireEvent.pointerDown(grip, { button: 0, clientX: 10, clientY: 100 });
  fireEvent.pointerMove(grip, { clientX: 10, clientY: 5 });
  expect(rows[0].className).toContain("message-queue-drop-before");
  fireEvent.pointerUp(grip, { clientX: 10, clientY: 5 });
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "reorder_message_queue",
      session_id: "s",
      expected: ["q", "b", "c"],
      order: ["c", "q", "b"],
    }),
  );
  await waitFor(() =>
    expect(
      within(screen.getByRole("list")).getAllByRole("listitem")[0].textContent,
    ).toContain("Third"),
  );
  expect(
    within(screen.getByRole("region", { name: "待发送消息" })).getByRole(
      "status",
    ).textContent,
  ).toBe("待发送顺序已保存");
});
it("supports keyboard reorder and ignores outside drops and edge arrow keys", async () => {
  mocks.request.mockResolvedValue({
    kind: "message_queue",
    entries: multiple(),
    paused: false,
    reason: null,
  });
  render(<MessageQueue {...props} />);
  const first = await screen.findByRole("button", {
    name: "排序排队消息：Original message",
  });
  fireEvent.keyDown(first, { key: "ArrowUp" });
  fireEvent.drop(screen.getByText("Third").closest("li")!);
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "reorder_message_queue",
    ),
  ).toHaveLength(0);
  fireEvent.keyDown(first, { key: "ArrowDown" });
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "reorder_message_queue",
      session_id: "s",
      expected: ["q", "b", "c"],
      order: ["b", "q", "c"],
    }),
  );
});
it("does not replay a failed reorder and refreshes a concurrently dispatched row", async () => {
  mocks.request.mockResolvedValue({
    kind: "message_queue",
    entries: multiple(),
    paused: false,
    reason: null,
  });
  render(<MessageQueue {...props} />);
  const first = await screen.findByRole("button", {
    name: "排序排队消息：Original message",
  });
  mocks.request.mockImplementation(async (_, request) => {
    if (request.method === "reorder_message_queue")
      throw new Error("队列已变化");
    return {
      kind: "message_queue",
      entries: multiple().slice(1),
      paused: false,
      reason: null,
    };
  });
  fireEvent.keyDown(first, { key: "ArrowDown" });
  await waitFor(() =>
    expect(screen.queryByText("Original message")).toBeNull(),
  );
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "reorder_message_queue",
    ),
  ).toHaveLength(1);
  expect(
    within(screen.getByRole("region", { name: "待发送消息" })).getByRole(
      "status",
    ).textContent,
  ).toBe("");
});

it("cancels pointer moves outside the list or on Escape without persisting", async () => {
  mocks.request.mockResolvedValue({
    kind: "message_queue",
    entries: multiple(),
    paused: false,
    reason: null,
  });
  render(<MessageQueue {...props} />);
  const grip = await screen.findByRole("button", {
    name: "排序排队消息：Third",
  });
  fireEvent.pointerDown(grip, { button: 0, clientX: 10, clientY: 100 });
  fireEvent.pointerMove(grip, { clientX: -10, clientY: -10 });
  fireEvent.pointerUp(grip);
  fireEvent.pointerDown(grip, { button: 0 });
  fireEvent.keyDown(grip, { key: "Escape" });
  fireEvent.pointerUp(grip);
  expect(
    mocks.request.mock.calls.filter(
      ([, r]) => r.method === "reorder_message_queue",
    ),
  ).toHaveLength(0);
});
it("restores keyboard focus after the server confirms each consecutive move", async () => {
  let pending = multiple();
  mocks.request.mockImplementation(async (_, request) => {
    if (request.method === "reorder_message_queue")
      pending = request.order.map((id: string) =>
        pending.find((item) => item.request_id === id)!,
      );
    return {
      kind: "message_queue",
      entries: pending,
      paused: false,
      reason: null,
    };
  });
  render(<MessageQueue {...props} />);
  let grip = await screen.findByRole("button", {
    name: "排序排队消息：Original message",
  });
  fireEvent.keyDown(grip, { key: "ArrowDown" });
  await waitFor(() => {
    expect(
      within(screen.getByRole("list")).getAllByRole("listitem")[1].textContent,
    ).toContain("Original message");
    expect(document.activeElement).toBe(grip);
  });
  grip = screen.getByRole("button", { name: "排序排队消息：Original message" });
  fireEvent.keyDown(grip, { key: "ArrowDown" });
  await waitFor(() => {
    expect(
      within(screen.getByRole("list")).getAllByRole("listitem")[2].textContent,
    ).toContain("Original message");
    expect(document.activeElement).toBe(grip);
  });
  expect(mocks.request).toHaveBeenCalledWith("ssh", {
    method: "reorder_message_queue",
    session_id: "s",
    expected: ["b", "q", "c"],
    order: ["b", "c", "q"],
  });
});

it("uses a send arrow without guide text, alongside delete and the edit menu", async () => {
  render(<MessageQueue {...props} onEdit={vi.fn()} />);
  const send = await screen.findByRole("button", {
    name: "发送排队消息：Original message",
  });
  expect(send.textContent).toBe("");
  expect(screen.queryByText("引导")).toBeNull();
  expect(
    screen.getByRole("button", { name: "删除排队消息：Original message" }),
  ).toBeTruthy();
  expect(
    screen.queryByRole("button", { name: "编辑排队消息：Original message" }),
  ).toBeNull();
});

it("sends without interrupt by default and requests interrupt only from the explicit menu action", async () => {
  render(<MessageQueue {...props} onEdit={vi.fn()} />);
  fireEvent.click(
    await screen.findByRole("button", {
      name: "发送排队消息：Original message",
    }),
  );
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "dispatch_queued_message",
      interrupt: false,
      session_id: "s",
      request_id: "q",
      revision: 3,
      provider: null,
    }),
  );
  const menu = screen.getByRole("button", {
    name: "排队消息更多操作：Original message",
  });
  await waitFor(() => expect((menu as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(menu);
  fireEvent.click(screen.getByRole("menuitem", { name: "停止当前并发送" }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "dispatch_queued_message",
      interrupt: true,
      session_id: "s",
      request_id: "q",
      revision: 3,
      provider: null,
    }),
  );
});
it("navigates the extra actions with arrow keys and restores the menu trigger on Escape", async () => {
  render(<MessageQueue {...props} onEdit={vi.fn()} />);
  const trigger = await screen.findByRole("button", {
    name: "排队消息更多操作：Original message",
  });
  fireEvent.click(trigger);
  expect(document.activeElement).toBe(
    screen.getByRole("menuitem", { name: "编辑消息" }),
  );
  fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
  expect(document.activeElement).toBe(
    screen.getByRole("menuitem", { name: "停止当前并发送" }),
  );
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});

it("recovers an ambiguous take after reconnect without replaying any prompt", async () => {
  const onEdit = vi.fn();
  let reject!: (cause: Error) => void;
  mocks.request.mockImplementation(async (_, command) =>
    command.method === "take_queued_message"
      ? new Promise((_, fail) => {
          reject = fail;
        })
      : {
          kind: "message_queue",
          entries: [entry],
          paused: false,
          reason: null,
        },
  );
  const view = render(<MessageQueue {...props} onEdit={onEdit} />);
  fireEvent.click(
    await screen.findByRole("button", {
      name: "排队消息更多操作：Original message",
    }),
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "编辑消息" }));
  view.rerender(<MessageQueue {...props} onEdit={onEdit} connected={false} />);
  await act(async () => reject(new Error("HOST_CONNECTION_LOST")));
  mocks.request.mockImplementation(async (_, command) =>
    command.method === "take_queued_message"
      ? { kind: "queued_message_taken", entry }
      : { kind: "message_queue", entries: [], paused: false, reason: null },
  );
  view.rerender(<MessageQueue {...props} onEdit={onEdit} />);
  fireEvent.click(await screen.findByRole("button", { name: /状态提示/ }));
  fireEvent.click(await screen.findByRole("button", { name: "恢复编辑内容" }));
  await waitFor(() => expect(onEdit).toHaveBeenCalledWith(entry));
  expect(
    mocks.request.mock.calls.every(
      ([, r]) => !["send", "send_queued"].includes(r.method),
    ),
  ).toBe(true);
});
it("restores a confirmed late take to its captured session after the queue unmounts", async () => {
  const onEdit = vi.fn();
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementation(async (_, command) =>
    command.method === "take_queued_message"
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : {
          kind: "message_queue",
          entries: [entry],
          paused: false,
          reason: null,
        },
  );
  const view = render(<MessageQueue {...props} onEdit={onEdit} />);
  fireEvent.click(
    await screen.findByRole("button", {
      name: "排队消息更多操作：Original message",
    }),
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "编辑消息" }));
  view.unmount();
  await act(async () => finish({ kind: "queued_message_taken", entry }));
  expect(onEdit).toHaveBeenCalledWith(entry);
});

it("rebases a drag onto the latest queue when the original head starts executing", async () => {
  let entries = multiple();
  mocks.request.mockImplementation(async () => ({
    kind: "message_queue",
    entries,
    paused: false,
    reason: null,
  }));
  const view = render(<MessageQueue {...props} />);
  const grip = await screen.findByRole("button", {
    name: "排序排队消息：Third",
  });
  const rows = screen.getAllByRole("listitem");
  vi.spyOn(screen.getByRole("list"), "getBoundingClientRect").mockReturnValue({
    left: 0,
    right: 300,
    top: 0,
    bottom: 120,
  } as DOMRect);
  rows.forEach((row, index) =>
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue({
      top: index * 40,
      bottom: (index + 1) * 40,
      height: 40,
    } as DOMRect),
  );
  fireEvent.pointerDown(grip, { button: 0, clientX: 10, clientY: 100 });
  fireEvent.pointerMove(grip, { clientX: 10, clientY: 45 });
  entries = entries.slice(1);
  view.rerender(<MessageQueue {...props} revision="head-dispatched" />);
  await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(2));
  fireEvent.pointerUp(grip);
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("ssh", {
      method: "reorder_message_queue",
      session_id: "s",
      expected: ["b", "c"],
      order: ["c", "b"],
    }),
  );
});
it("closes the portal on Tab without leaving focus on a removed menu item", async () => {
  render(<MessageQueue {...props} onEdit={vi.fn()} />);
  const trigger = await screen.findByRole("button", {
    name: "排队消息更多操作：Original message",
  });
  fireEvent.click(trigger);
  expect(document.activeElement).toBe(
    screen.getByRole("menuitem", { name: "编辑消息" }),
  );
  fireEvent.keyDown(document, { key: "Tab" });
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
