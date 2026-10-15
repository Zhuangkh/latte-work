// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useSubagents } from "./useSubagents";
import type { Subagent } from "./protocol";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  mocks.request.mockReset();
});
const task: Subagent = {
  id: "child",
  native_id: "child",
  title: "检查",
  tool_use_id: null,
  status: "running",
  summary: null,
  last_tool: null,
  started_at: 1,
  updated_at: 1,
};
it("ignores late snapshots after host/session switching and marks disconnected live evidence unknown", async () => {
  let first!: (value: unknown) => void;
  mocks.request
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          first = resolve;
        }),
    )
    .mockResolvedValue({ kind: "subagents", tasks: [task], truncated: false });
  const hook = renderHook(
    ({ host, session, connected }) =>
      useSubagents(host, session, connected, []),
    { initialProps: { host: "local", session: "one", connected: true } },
  );
  hook.rerender({ host: "remote", session: "two", connected: true });
  await act(async () => {});
  expect(hook.result.current.tasks[0].id).toBe("child");
  await act(async () =>
    first({
      kind: "subagents",
      tasks: [{ ...task, id: "stale" }],
      truncated: false,
    }),
  );
  expect(hook.result.current.tasks[0].id).toBe("child");
  hook.rerender({ host: "remote", session: "two", connected: false });
  expect(hook.result.current.tasks[0].status).toBe("unknown");
  expect(mocks.request).toHaveBeenCalledTimes(2);
});

it("refreshes a visible list without parent events, stops periodic reads when hidden, and resumes immediately", async () => {
  vi.useFakeTimers();
  mocks.request
    .mockResolvedValueOnce({
      kind: "subagents",
      tasks: [task],
      truncated: false,
    })
    .mockResolvedValue({
      kind: "subagents",
      tasks: [{ ...task, status: "completed" }],
      truncated: false,
    });
  const hook = renderHook(
    ({ visible }) => useSubagents("local", "s", true, [], 0, visible),
    { initialProps: { visible: true } },
  );
  await act(async () => {});
  expect(hook.result.current.tasks[0].status).toBe("running");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(650);
  });
  expect(hook.result.current.tasks[0].status).toBe("completed");
  hook.rerender({ visible: false });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(10000);
  });
  expect(mocks.request).toHaveBeenCalledTimes(2);
  hook.rerender({ visible: true });
  await act(async () => {});
  expect(mocks.request).toHaveBeenCalledTimes(3);
});
it("queues event refreshes behind a slow snapshot, and fences old connection snapshots", async () => {
  let resolve!: (v: unknown) => void;
  mocks.request
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r;
        }),
    )
    .mockResolvedValue({ kind: "subagents", tasks: [task], truncated: false });
  const hook = renderHook(
    ({ revision, version }) =>
      useSubagents(
        "local",
        "s",
        true,
        [
          {
            session_id: "s",
            seq: revision,
            at: 0,
            event: {
              kind: "subagent",
              update: {
                id: "child",
                status: "running",
                tool_use_id: null,
                title: null,
                summary: null,
                last_tool: null,
              },
            },
          },
        ],
        version,
        false,
      ),
    { initialProps: { revision: 0, version: 0 } },
  );
  hook.rerender({ revision: 1, version: 0 });
  hook.rerender({ revision: 2, version: 0 });
  expect(mocks.request).toHaveBeenCalledTimes(1);
  hook.rerender({ revision: 2, version: 1 });
  await act(async () => {});
  await act(async () =>
    resolve({ kind: "subagents", tasks: [], truncated: false }),
  );
  expect(hook.result.current.tasks).toEqual([task]);
  expect(mocks.request).toHaveBeenCalledTimes(2);
});
