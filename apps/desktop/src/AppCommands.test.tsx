// @vitest-environment jsdom
import { useState } from "react";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Conversation } from "./Conversation";
import { DraftStore } from "./drafts";
import { renderWithStatus } from "./test/renderWithStatus";
import type { Effort, Event, Session } from "./protocol";
const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  model: vi.fn(),
  effort: vi.fn(),
}));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
vi.mock("./PermissionPicker", () => ({ PermissionPicker: () => null }));
vi.mock("./TaskProjectPicker", () => ({ TaskProjectPicker: () => null }));
vi.mock("./useAgentPreferences", () => ({
  useAgentPreferences: () => {
    const [model, setModel] = useState<string | null>("opus");
    const [effort, setEffort] = useState<Effort | null>("high");
    return {
      model,
      effort,
      permissionMode: null,
      chooseModel: (value: string | null) => {
        mocks.model(value);
        setModel(value);
      },
      chooseEffort: (value: Effort | null) => {
        mocks.effort(value);
        setEffort(value);
      },
    };
  },
}));
beforeEach(() => {
  vi.clearAllMocks();
  HTMLElement.prototype.scrollTo = vi.fn();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  mocks.request.mockResolvedValue({
    kind: "models",
    models: ["opus", "haiku"],
    provider: null,
    default_model: null,
    effort_levels: ["low", "high"],
    model_labels: {},
  });
});
afterEach(cleanup);
function setup(extra = {}) {
  const p = {
    drafts: new DraftStore(),
    hostId: "remote",
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
    ...extra,
  };
  renderWithStatus(<Conversation {...p} />);
  return p;
}
const input = () =>
  screen.getByRole("textbox", { name: "任务输入" }) as HTMLTextAreaElement;
function command(text: string) {
  // Trailing whitespace matches the command inserted by the autocomplete menu.
  fireEvent.change(input(), { target: { value: `${text} ` } });
  fireEvent.keyDown(input(), { key: "Enter" });
}
it.each(["model", "effort"] as const)(
  "/%s opens the shared menu and updates App preferences without a turn",
  async (target) => {
    const p = setup();
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Model and reasoning effort" })
          .hasAttribute("disabled"),
      ).toBe(false),
    );
    mocks.request.mockClear();
    command(`/${target}`);
    expect(input().value).toBe("");
    const menu = screen.getByRole("menu", {
      name: target === "model" ? "Model" : "Reasoning effort",
    });
    expect(menu.contains(document.activeElement)).toBe(true);
    const option = screen.getByRole("menuitemradio", {
      name: target === "model" ? "haiku" : "low",
    });
    // The real portal keyboard handler must not bubble into send or stop.
    option.focus();
    fireEvent.keyDown(option, { key: "Enter" });
    expect(
      target === "model" ? mocks.model : mocks.effort,
    ).toHaveBeenCalledWith(target === "model" ? "haiku" : "low");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(p.send).not.toHaveBeenCalled();
    expect(p.cancel).not.toHaveBeenCalled();
    expect(
      mocks.request.mock.calls.some(([, r]) =>
        ["send", "create_session", "agent_commands"].includes(r.method),
      ),
    ).toBe(false);
  },
);
it("keeps /model from becoming stop during a running turn", async () => {
  const events: Event[] = [
    {
      seq: 1,
      at: 1,
      session_id: "s",
      event: { kind: "user", request_id: "r", text: "work" },
    },
  ];
  const p = setup({
    session: {
      id: "s",
      agent: "claude",
      status: "running",
      archived: false,
    } as Session,
    events,
  });
  await waitFor(() => expect(mocks.request).toHaveBeenCalled());
  command("/model");
  expect(screen.getByText(/请在任务结束后选择/)).toBeTruthy();
  expect(input().value).toBe("/model ");
  expect(screen.queryByRole("menu")).toBeNull();
  expect(p.send).not.toHaveBeenCalled();
  expect(p.cancel).not.toHaveBeenCalled();
});
it("preserves /effort when the selected model has no effort capability", async () => {
  mocks.request.mockResolvedValue({
    kind: "models",
    models: ["opus"],
    provider: null,
    default_model: null,
    effort_levels: [],
    model_labels: {},
  });
  const p = setup();
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "Model and reasoning effort" })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  command("/effort");
  expect(screen.getByText(/不支持调整思考强度/)).toBeTruthy();
  expect(input().value).toBe("/effort ");
  expect(p.send).not.toHaveBeenCalled();
});
it("preserves a model command during disconnect without opening a stale menu", () => {
  const p = setup({ connected: false });
  command("/model");
  expect(screen.getByText(/主机未连接/)).toBeTruthy();
  expect(input().value).toBe("/model ");
  expect(screen.queryByRole("menu")).toBeNull();
  expect(p.send).not.toHaveBeenCalled();
});
