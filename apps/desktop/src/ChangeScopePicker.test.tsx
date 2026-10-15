// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ChangeScopePicker } from "./ChangeScopePicker";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const props = {
  hostId: "local",
  projectId: "p",
  scope: "lastTurn" as const,
  disabled: false,
  choose: vi.fn(),
};
beforeEach(() => {
  vi.clearAllMocks();
  mocks.request.mockReset();
});
afterEach(cleanup);
function openCommits() {
  fireEvent.click(screen.getByRole("combobox", { name: "变更范围" }));
  fireEvent.click(screen.getByRole("option", { name: "已提交" }));
}
it("loads commit pages only on demand, keeps the head cursor, searches and restores trigger focus", async () => {
  const head = "a".repeat(40),
    older = "b".repeat(40);
  mocks.request
    .mockResolvedValueOnce({
      kind: "git_commits",
      head,
      commits: [{ oid: head, subject: "Newest" }],
      has_more: true,
    })
    .mockResolvedValueOnce({
      kind: "git_commits",
      head,
      commits: [{ oid: older, subject: "Older" }],
      has_more: false,
    });
  render(<ChangeScopePicker {...props} />);
  expect(mocks.request).not.toHaveBeenCalled();
  openCommits();
  await screen.findByRole("option", { name: /Newest/ });
  fireEvent.click(screen.getByRole("button", { name: "加载更早的提交" }));
  await screen.findByRole("option", { name: /Older/ });
  expect(mocks.request).toHaveBeenLastCalledWith("local", {
    method: "git_commits",
    project_id: "p",
    head,
    offset: 1,
  });
  fireEvent.change(screen.getByRole("textbox", { name: "搜索提交" }), {
    target: { value: "older" },
  });
  expect(screen.queryByRole("option", { name: /Newest/ })).toBeNull();
  fireEvent.keyDown(screen.getByRole("textbox", { name: "搜索提交" }), {
    key: "ArrowDown",
  });
  expect(document.activeElement).toBe(
    screen.getByRole("option", { name: /Older/ }),
  );
  fireEvent.click(screen.getByRole("option", { name: /Older/ }));
  expect(props.choose).toHaveBeenCalledWith("commit", {
    oid: older,
    subject: "Older",
  });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(document.activeElement).toBe(
    screen.getByRole("combobox", { name: "变更范围" }),
  );
});
it("does not replace a closed or changed host menu with a late response and explicitly retries failures", async () => {
  let finish!: (value: unknown) => void;
  mocks.request
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = done;
        }),
    )
    .mockRejectedValueOnce(new Error("Offline"))
    .mockResolvedValueOnce({
      kind: "git_commits",
      head: null,
      commits: [],
      has_more: false,
    });
  const view = render(<ChangeScopePicker {...props} />);
  openCommits();
  view.rerender(<ChangeScopePicker {...props} hostId="remote" />);
  expect(screen.queryByRole("listbox")).toBeNull();
  await act(async () =>
    finish({
      kind: "git_commits",
      head: "a".repeat(40),
      commits: [{ oid: "a".repeat(40), subject: "STALE" }],
      has_more: false,
    }),
  );
  openCommits();
  await screen.findByRole("alert");
  expect(screen.queryByText("STALE")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  await screen.findByText("仓库尚无提交");
  expect(mocks.request.mock.calls).toHaveLength(3);
});
it("supports keyboard submenu navigation and closes the picker when disabled", async () => {
  mocks.request.mockResolvedValue({
    kind: "git_commits",
    head: null,
    commits: [],
    has_more: false,
  });
  const view = render(<ChangeScopePicker {...props} />);
  fireEvent.click(screen.getByRole("combobox", { name: "变更范围" }));
  const item = screen.getByRole("option", { name: "已提交" });
  item.focus();
  fireEvent.keyDown(item, { key: "ArrowRight" });
  await screen.findByText("仓库尚无提交");
  expect(document.activeElement).toBe(
    screen.getByRole("textbox", { name: "搜索提交" }),
  );
  fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
  await waitFor(() => expect(screen.queryByRole("textbox")).toBeNull());
  expect(document.activeElement).toBe(
    screen.getByRole("option", { name: "已提交" }),
  );
  fireEvent.keyDown(window, { key: "Escape" });
  expect(document.activeElement).toBe(
    screen.getByRole("combobox", { name: "变更范围" }),
  );
  openCommits();
  view.rerender(<ChangeScopePicker {...props} disabled />);
  expect(screen.queryByRole("listbox")).toBeNull();
});
