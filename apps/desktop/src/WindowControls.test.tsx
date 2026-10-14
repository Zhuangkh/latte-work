// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WindowControls, windowControlsVisible } from "./WindowControls";

const windowMock = vi.hoisted(() => ({
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  isMaximized: vi.fn().mockResolvedValue(false),
  onResized: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => windowMock,
}));

describe("WindowControls", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    windowMock.isMaximized.mockResolvedValue(false);
    windowMock.onResized.mockResolvedValue(() => {});
  });
  it("uses native window controls on Windows and Linux only", () => {
    expect(windowControlsVisible("windows")).toBe(true);
    expect(windowControlsVisible("linux")).toBe(true);
    expect(windowControlsVisible("macos")).toBe(false);
    expect(windowControlsVisible(undefined)).toBe(false);
  });
  it("renders nothing where the platform keeps native decorations", () => {
    const view = render(<WindowControls platform="macos" />);
    expect(view.container.textContent).toBe("");
  });
  it("drives minimize, maximize and close through the window API", async () => {
    render(<WindowControls platform="windows" />);
    fireEvent.click(screen.getByRole("button", { name: "最小化" }));
    fireEvent.click(screen.getByRole("button", { name: "最大化" }));
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(windowMock.minimize).toHaveBeenCalledTimes(1);
    expect(windowMock.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(windowMock.close).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "最大化" })).toBeTruthy();
  });
  it("shows the restore affordance while maximized", async () => {
    windowMock.isMaximized.mockResolvedValue(true);
    render(<WindowControls platform="windows" />);
    expect(await screen.findByRole("button", { name: "还原" })).toBeTruthy();
  });
});
