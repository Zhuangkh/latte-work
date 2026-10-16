// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SubagentsPanel } from "./SubagentsPanel";
import type { Subagent } from "./protocol";
afterEach(cleanup);
vi.mock("./api", () => ({
  request: vi.fn(async () => ({
    kind: "subagent_history",
    events: [],
    incomplete: false,
    has_more: false,
    source_has_more: false,
    source: "stream",
    after: null,
  })),
  message: String,
}));
const task = (id: string, status: Subagent["status"]): Subagent => ({
  id,
  native_id: id,
  tool_use_id: null,
  title: id,
  status,
  summary: status === "completed" ? "结果已返回" : null,
  last_tool: "Read",
  started_at: 1,
  updated_at: 2000,
});
it("separates running/completed/unknown and shows a selected native result", () => {
  const state = {
    tasks: [
      task("running child", "running"),
      task("completed child", "completed"),
      task("lost child", "unknown"),
    ],
    loading: false,
    error: "",
    truncated: false,
  };
  const select = vi.fn();
  const view = render(<SubagentsPanel state={state} onOpen={select} />);
  expect(screen.getByRole("region", { name: "已开启" }).textContent).toContain(
    "running child",
  );
  expect(screen.getByRole("region", { name: "完成" }).textContent).toContain(
    "completed child",
  );
  fireEvent.click(screen.getByRole("button", { name: "完成 · 1" }));
  fireEvent.click(screen.getByRole("button", { name: /completed child/ }));
  expect(select).toHaveBeenCalledWith(state.tasks[1]);
  expect(screen.getByRole("region", { name: "已开启" })).toBeTruthy();
  view.rerender(<SubagentsPanel state={state} taskId="completed child" />);
  expect(screen.getByText("结果已返回")).toBeTruthy();
  expect(screen.queryByText("任务标识")).toBeNull();
  expect(
    screen.queryByText("completed child", { selector: "code" }),
  ).toBeNull();
  view.rerender(
    <SubagentsPanel
      state={{ ...state, tasks: [task("other child", "running")] }}
      onOpen={select}
    />,
  );
  expect(screen.queryByText("结果已返回")).toBeNull();
  expect(screen.getByRole("button", { name: /other child/ })).toBeTruthy();
});

it("shows native roles below names and current tools only while running", () => {
  const select = vi.fn();
  const role = "review:code-reviewer";
  const state = {
    tasks: [
      { ...task("Review service", "running"), agent_type: role },
      { ...task("Review frontend", "completed"), agent_type: role },
      { ...task("Paused", "paused"), agent_type: "Explore" },
      task("Legacy", "unknown"),
      { ...task("No role", "failed"), agent_type: "  " },
    ],
    loading: false,
    error: "",
    truncated: false,
  };
  render(<SubagentsPanel state={state} onOpen={select} />);
  fireEvent.click(screen.getByRole("button", { name: "完成 · 1" }));
  const running = screen.getByRole("button", { name: /Review service/ });
  expect(running.querySelector(".subagent-metadata")?.textContent).toBe(
    `${role} · 处理中 · Read`,
  );
  expect(running.querySelector(".subagent-type")?.getAttribute("title")).toBe(
    role,
  );
  for (const [name, metadata] of [
    ["Review frontend", `${role} · 已完成`],
    ["Paused", "Explore · 已暂停"],
    ["Legacy", "状态待确认"],
    ["No role", "执行失败"],
  ]) {
    const row = screen.getByRole("button", { name: new RegExp(name) });
    expect(row.querySelector(".subagent-metadata")?.textContent).toBe(metadata);
  }
  running.focus();
  expect(document.activeElement).toBe(running);
  fireEvent.click(running);
  expect(select).toHaveBeenCalledWith(state.tasks[0]);
});

it("uses the conversation waiting row without a composer or duplicate metadata header", async () => {
  const state = {
    tasks: [{ ...task("child", "running"), last_tool: null }],
    loading: false,
    error: "",
    truncated: false,
  };
  const view = render(
    <SubagentsPanel
      state={state}
      taskId="child"
      hostId="local"
      sessionId="s"
      active
    />,
  );
  await act(async () => {});
  expect(screen.getByRole("status").textContent).toContain("等待响应");
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByText(/最近工具|尚未收到过程输出/)).toBeNull();
  view.rerender(
    <SubagentsPanel
      state={{ ...state, tasks: [task("child", "completed")] }}
      taskId="child"
      hostId="local"
      sessionId="s"
      active
    />,
  );
  await act(async () => {});
  expect(screen.queryByRole("status")).toBeNull();
  expect(screen.getByText("结果已返回")).toBeTruthy();
});
it("preserves review position across automatic reads and offers return-to-latest", async () => {
  const state = {
    tasks: [task("child", "running")],
    loading: false,
    error: "",
    truncated: false,
  };
  const view = render(
    <SubagentsPanel
      state={state}
      taskId="child"
      hostId="local"
      sessionId="s"
      active
    />,
  );
  await act(async () => {});
  const scroll = document.querySelector(
    ".subagent-conversation .conversation-scroll",
  ) as HTMLDivElement;
  Object.defineProperties(scroll, {
    clientHeight: { value: 300, configurable: true },
    scrollHeight: { value: 1500, configurable: true },
    scrollTop: { value: 100, writable: true, configurable: true },
  });
  scroll.scrollTo = vi.fn();
  fireEvent.scroll(scroll);
  expect(screen.getByRole("button", { name: "回到最新" })).toBeTruthy();
  view.rerender(
    <SubagentsPanel
      state={{
        ...state,
        tasks: [{ ...task("child", "running"), updated_at: 3000 }],
      }}
      taskId="child"
      hostId="local"
      sessionId="s"
      active
    />,
  );
  await act(async () => {});
  expect(scroll.scrollTop).toBe(100);
  fireEvent.click(screen.getByRole("button", { name: "回到最新" }));
  expect(scroll.scrollTo).toHaveBeenCalledWith({ top: 1500 });
  expect(screen.queryByRole("button", { name: "回到最新" })).toBeNull();
});

it("defaults completed children to collapsed, keeps active and uncertain tasks visible, and preserves explicit expansion", () => {
  const state = {
    tasks: [
      task("active-review", "running"),
      task("done-review", "completed"),
      task("uncertain-review", "unknown"),
    ],
    loading: false,
    error: "",
    truncated: false,
  };
  const view = render(
    <SubagentsPanel
      state={state}
      hostId="collapse-test"
      sessionId="disclosure-session"
    />,
  );
  const done = screen.getByRole("button", { name: "完成 · 1" });
  expect(done.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("button", { name: /done-review/ })).toBeNull();
  expect(screen.getByRole("button", { name: /active-review/ })).toBeTruthy();
  expect(screen.getByRole("button", { name: /uncertain-review/ })).toBeTruthy();
  fireEvent.click(done);
  expect(screen.getByRole("button", { name: /done-review/ })).toBeTruthy();
  view.rerender(
    <SubagentsPanel
      state={{
        ...state,
        tasks: [...state.tasks, task("done-second", "completed")],
      }}
      hostId="collapse-test"
      sessionId="disclosure-session"
    />,
  );
  expect(
    screen
      .getByRole("button", { name: "完成 · 2" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  expect(screen.getByRole("button", { name: /done-second/ })).toBeTruthy();
  view.rerender(
    <SubagentsPanel
      state={state}
      hostId="collapse-test"
      sessionId="different-session"
    />,
  );
  expect(
    screen
      .getByRole("button", { name: "完成 · 1" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
});
