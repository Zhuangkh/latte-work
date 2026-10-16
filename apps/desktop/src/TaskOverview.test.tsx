// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { TaskOverview } from "./TaskOverview";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
afterEach(cleanup);
const project = { id: "p", name: "Project", path: "/project" };
const state = {
  tasks: [
    {
      id: "child",
      native_id: "native",
      tool_use_id: null,
      title: "Child",
      status: "running" as const,
      summary: null,
      last_tool: "Read",
      started_at: 1,
      updated_at: 2,
    },
  ],
  loading: false,
  error: "",
  truncated: false,
};
it("opens a keyboard-accessible overview and routes to real changes, child list without sources", async () => {
  mocks.request.mockResolvedValue({
    kind: "git_review",
    review: {
      entries: [{ path: "a" }, { path: "b" }],
      added: 12,
      removed: 3,
      binary_files: 0,
      truncated: false,
      unavailable: null,
    },
  });
  const changes = vi.fn(),
    children = vi.fn();
  render(
    <TaskOverview
      hostId="local"
      project={project}
      connected
      subagents={state}
      openChanges={changes}
      openSubagents={children}
      openInfo={vi.fn()}
    />,
  );
  const trigger = screen.getByRole("button", { name: "任务概览" });
  fireEvent.click(trigger);
  await screen.findByText("2 个文件");
  expect(screen.queryByText("来源")).toBeNull();
  expect(
    screen.queryByRole("button", { name: /添加来源|查看全部/ }),
  ).toBeNull();
  expect(screen.getByText("+12")).toBeTruthy();
  expect(screen.getByText("-3")).toBeTruthy();
  expect(screen.getByText("1 个运行中 · 0 完成")).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
  fireEvent.click(trigger);
  fireEvent.click(screen.getByRole("button", { name: /变更/ }));
  expect(changes).toHaveBeenCalledOnce();
  fireEvent.click(trigger);
  fireEvent.click(screen.getByRole("button", { name: /1 个运行中/ }));
  expect(children).toHaveBeenCalledOnce();
});
it("fences late changes and scopes the overview to its host/project", async () => {
  let resolve!: (value: unknown) => void;
  mocks.request
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    )
    .mockResolvedValue({
      kind: "git_review",
      review: {
        entries: [],
        added: 0,
        removed: 0,
        binary_files: 0,
        truncated: false,
        unavailable: null,
      },
    });
  const props = {
    connected: true,
    subagents: state,
    openChanges: vi.fn(),
    openSubagents: vi.fn(),
    openInfo: vi.fn(),
  };
  const view = render(
    <TaskOverview {...props} hostId="local" project={project} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "任务概览" }));
  view.rerender(
    <TaskOverview
      {...props}
      hostId="remote"
      project={{ ...project, id: "other" }}
    />,
  );
  await screen.findByText("0 个文件");
  resolve({
    kind: "git_review",
    review: {
      entries: [{ path: "stale" }],
      added: 20,
      removed: 0,
      truncated: false,
    },
  });
  await waitFor(() => expect(screen.getByText("0 个文件")).toBeTruthy());
});
it("pins the overview into task information and dismisses the popover", () => {
  const openInfo = vi.fn();
  render(
    <TaskOverview
      hostId="local"
      connected={false}
      subagents={state}
      openChanges={vi.fn()}
      openSubagents={vi.fn()}
      openInfo={openInfo}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "任务概览" }));
  const pin = screen.getByRole("button", { name: "固定到右侧栏" });
  expect(document.activeElement).toBe(pin);
  fireEvent.click(pin);
  expect(openInfo).toHaveBeenCalledOnce();
  expect(screen.queryByRole("dialog")).toBeNull();
});
