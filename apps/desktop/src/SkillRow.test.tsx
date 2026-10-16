// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SkillRow } from "./SkillRow";
import { readWorkspace } from "./workspaceState";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
const info = {
  name: "Code Review",
  description: "Review code changes using evidence.",
  directory: "/host/skills/review",
};
const props = {
  name: "review",
  uses: 2,
  workspaceId: "",
  hostId: "remote",
  sessionId: "s",
  project: { id: "p", name: "P", path: "/project" },
  connected: true,
  active: true,
};
beforeEach(() => {
  props.workspaceId = crypto.randomUUID();
  mocks.request.mockReset().mockImplementation(async (_host, req) =>
    req.method === "skill_info"
      ? { kind: "skill_info", skill: info }
      : {
          kind: "file_reference",
          entry: { name: "review", path: info.directory, directory: true },
        },
  );
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
it("loads real hover details without opening an Agent or granting file access", async () => {
  vi.useFakeTimers();
  render(<SkillRow {...props} />);
  const row = screen.getByRole("button", { name: "查看 Skill review" });
  expect(mocks.request).not.toHaveBeenCalled();
  fireEvent.mouseEnter(row);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(350);
  });
  expect(screen.getByRole("tooltip").textContent).toContain(info.description);
  expect(screen.getByRole("tooltip").textContent).toContain(info.name);
  expect(screen.getByRole("tooltip").textContent).toContain(info.directory);
  expect(mocks.request).toHaveBeenCalledExactlyOnceWith("remote", {
    method: "skill_info",
    session_id: "s",
    name: "review",
  });
  expect(readWorkspace(props.workspaceId).tabs).toEqual([]);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("tooltip")).toBeNull();
});
it("opens the selected host's actual skill directory only after an explicit click", async () => {
  render(<SkillRow {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "查看 Skill review" }));
  await waitFor(() =>
    expect(readWorkspace(props.workspaceId).tabs).toHaveLength(1),
  );
  expect(mocks.request).toHaveBeenCalledWith("remote", {
    method: "resolve_reference",
    project_id: "p",
    path: info.directory,
  });
  expect(readWorkspace(props.workspaceId).tabs[0]).toMatchObject({
    kind: "files",
    file: `${info.directory}/SKILL.md`,
    reference: { name: info.name, path: info.directory, directory: true },
    hostId: "remote",
    sessionId: "s",
  });
});
it("never opens a directory from a late response after navigation", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<SkillRow {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "查看 Skill review" }));
  view.rerender(<SkillRow {...props} active={false} />);
  await act(async () => finish({ kind: "skill_info", skill: info }));
  expect(mocks.request).toHaveBeenCalledTimes(1);
  expect(readWorkspace(props.workspaceId).tabs).toEqual([]);
});
it("does not synthesize metadata or clickable paths for unavailable skills", async () => {
  mocks.request.mockResolvedValue({ kind: "skill_info", skill: null });
  render(<SkillRow {...props} />);
  const row = screen.getByRole("button", { name: "查看 Skill review" });
  fireEvent.focus(row);
  await waitFor(() => expect(row.getAttribute("aria-disabled")).toBe("true"));
  fireEvent.click(row);
  expect(mocks.request).toHaveBeenCalledTimes(1);
  expect(readWorkspace(props.workspaceId).tabs).toEqual([]);
});
