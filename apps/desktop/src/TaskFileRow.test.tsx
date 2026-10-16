// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { TaskFileRow } from "./TaskFileRow";
import { readWorkspace } from "./workspaceState";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const props = {
  file: { path: "/tmp/report.md", name: "Report", directory: false },
  workspaceId: "",
  hostId: "ssh",
  project: { id: "p", name: "Project", path: "/project" },
  sessionId: "s",
  active: true,
  connected: true,
};
beforeEach(() => {
  props.workspaceId = crypto.randomUUID();
  mocks.request.mockReset();
});
afterEach(cleanup);
it("resolves only an explicitly clicked external file and opens the shared file tab", async () => {
  mocks.request.mockResolvedValue({
    kind: "file_reference",
    entry: { ...props.file, path: "/private/tmp/report.md" },
  });
  render(<TaskFileRow {...props} />);
  expect(mocks.request).not.toHaveBeenCalled();
  expect(screen.getByText(props.file.path)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Report/ }));
  await waitFor(() =>
    expect(readWorkspace(props.workspaceId).current).toBe("files"),
  );
  expect(mocks.request).toHaveBeenCalledExactlyOnceWith("ssh", {
    method: "resolve_reference",
    project_id: "p",
    path: "/tmp/report.md",
  });
  expect(readWorkspace(props.workspaceId).tabs[0]).toMatchObject({
    kind: "files",
    file: "/private/tmp/report.md",
    reference: { path: "/private/tmp/report.md", directory: false },
    hostId: "ssh",
    sessionId: "s",
  });
});
it("discards a late selection after leaving and returning to the same task", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<TaskFileRow {...props} />);
  fireEvent.click(screen.getByRole("button", { name: /Report/ }));
  view.rerender(<TaskFileRow {...props} active={false} />);
  view.rerender(<TaskFileRow {...props} />);
  await act(async () => finish({ kind: "file_reference", entry: props.file }));
  expect(readWorkspace(props.workspaceId).tabs).toEqual([]);
  expect(
    screen.getByRole("button", { name: /Report/ }).hasAttribute("disabled"),
  ).toBe(false);
});
it("keeps missing files out of the file browser and reports a retryable failure", async () => {
  mocks.request.mockRejectedValue(new Error("文件不存在"));
  render(<TaskFileRow {...props} />);
  fireEvent.click(screen.getByRole("button", { name: /Report/ }));
  fireEvent.click(await screen.findByRole("button", { name: /状态提示/ }));
  expect(await screen.findByText("Error: 文件不存在")).toBeTruthy();
  expect(readWorkspace(props.workspaceId).tabs).toEqual([]);
});
