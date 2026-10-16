// @vitest-environment jsdom
import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useState } from "react";
import { Sidebar } from "./Sidebar";
import {
  projectOrderKey as key,
  moveProjectOrder,
  orderProjects,
} from "./projectOrder";
import type { Workbench } from "./useWorkbench";
import { renderWithStatus as render } from "./test/renderWithStatus";
vi.mock("./api", () => ({
  request: vi.fn().mockResolvedValue({ kind: "sessions", sessions: [] }),
  message: String,
}));
const projects = [
  { id: "a", hostId: "local", name: "Alpha", path: "/a" },
  { id: "b", hostId: "devbox", name: "Beta", path: "/b" },
  { id: "c", hostId: "local", name: "Gamma", path: "/c" },
];
const move = vi.fn();
function Harness({
  visible = true,
  items = projects,
}: {
  visible?: boolean;
  items?: typeof projects;
}) {
  const [order, setOrder] = useState<string[]>([]);
  return (
    <Sidebar
      state={
        {
          hosts: [
            { id: "local", name: "本机" },
            { id: "devbox", name: "devbox" },
          ],
          hostId: "local",
          connected: true,
          projects: orderProjects(items, order),
          projectId: "",
          sessions: [],
          sessionId: "",
          pinned: [],
          hostErrors: {},
          sidebarOpen: visible,
          moveProject: (source: string, target: string, after: boolean) => {
            move(source, target, after);
            setOrder((old) =>
              moveProjectOrder(old, items, source, target, after),
            );
          },
        } as unknown as Workbench
      }
    />
  );
}
beforeEach(() => {
  localStorage.clear();
  localStorage.setItem(
    "latte-work.sidebar-sections.v1",
    JSON.stringify({ recent: false }),
  );
  vi.stubGlobal("PointerEvent", MouseEvent);
  move.mockReset();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
function bounds() {
  const rect = (top: number, height: number) =>
    ({
      left: 0,
      right: 240,
      top,
      bottom: top + height,
      width: 240,
      height,
    }) as DOMRect;
  const list = document.getElementById("sidebar-projects")!;
  vi.spyOn(list, "getBoundingClientRect").mockReturnValue(rect(100, 120));
  vi.spyOn(
    list.closest(".sidebar-navigation")!,
    "getBoundingClientRect",
  ).mockReturnValue(rect(50, 300));
  list
    .querySelectorAll<HTMLElement>(".project-group")
    .forEach((row, i) =>
      vi
        .spyOn(row, "getBoundingClientRect")
        .mockReturnValue(rect(100 + i * 40, 40)),
    );
  return list;
}
function drag(name = "Gamma", y = 105) {
  const row = screen.getByRole("button", { name });
  fireEvent.pointerDown(row, { button: 0, clientX: 80, clientY: 200 });
  fireEvent.pointerMove(window, { buttons: 1, clientX: 80, clientY: y });
  return row;
}
it("shows a floating project and insertion line, then moves across hosts without expanding", () => {
  render(<Harness />);
  bounds();
  const row = drag();
  expect(
    document.querySelector(".project-drop-indicator")?.getAttribute("style"),
  ).toContain("top: 100px");
  expect(document.querySelector(".project-drag-preview")?.textContent).toBe(
    "Gamma",
  );
  expect(row.getAttribute("aria-expanded")).toBe("false");
  fireEvent.pointerUp(window, { clientX: 80, clientY: 105 });
  fireEvent.click(row);
  expect(move).toHaveBeenCalledWith(key(projects[2]), key(projects[0]), false);
  expect(
    [...document.querySelectorAll(".project-name")].map((n) => n.textContent),
  ).toEqual(["Gamma", "Alpha", "Beta"]);
  expect(row.getAttribute("aria-expanded")).toBe("false");
  expect(document.querySelector(".project-drop-indicator")).toBeNull();
});
it.each(["escape", "outside", "cancel", "blur", "lost-capture"])(
  "cancels %s without changing the saved order",
  (reason) => {
    render(<Harness />);
    bounds();
    const row = drag();
    if (reason === "escape") fireEvent.keyDown(window, { key: "Escape" });
    if (reason === "outside")
      fireEvent.pointerUp(window, { clientX: 500, clientY: 105 });
    if (reason === "cancel") fireEvent.pointerCancel(window);
    if (reason === "blur") fireEvent.blur(window);
    if (reason === "lost-capture") fireEvent.lostPointerCapture(row);
    expect(move).not.toHaveBeenCalled();
    expect(document.querySelector(".project-drag-preview")).toBeNull();
  },
);
it("keeps a click and tiny pointer movement as the existing expand action", () => {
  render(<Harness />);
  bounds();
  const row = screen.getByRole("button", { name: "Alpha" });
  const capture = vi.fn();
  row.setPointerCapture = capture;
  fireEvent.pointerDown(row, { button: 0, clientX: 80, clientY: 110 });
  fireEvent.pointerMove(window, { buttons: 1, clientX: 81, clientY: 111 });
  expect(capture).not.toHaveBeenCalled();
  fireEvent.pointerUp(window, { clientX: 81, clientY: 111 });
  fireEvent.click(row);
  expect(row.getAttribute("aria-expanded")).toBe("true");
  expect(move).not.toHaveBeenCalled();
});
it("supports keyboard reordering while retaining focus and expanded contents", async () => {
  render(<Harness />);
  const row = screen.getByRole("button", { name: "Beta" });
  fireEvent.click(row);
  row.focus();
  fireEvent.keyDown(row, { key: "ArrowUp", altKey: true });
  await act(async () => {});
  expect(move).toHaveBeenCalledWith(key(projects[1]), key(projects[0]), false);
  expect(document.activeElement).toBe(row);
  expect(row.getAttribute("aria-expanded")).toBe("true");
  expect(
    [...document.querySelectorAll(".project-name")].map((n) => n.textContent),
  ).toEqual(["Beta", "Alpha", "Gamma"]);
});
it("cancels when the sidebar is hidden or the dragged project is removed", () => {
  const view = render(<Harness />);
  bounds();
  drag();
  view.rerender(<Harness visible={false} />);
  expect(document.querySelector(".project-drag-preview")).toBeNull();
  view.rerender(<Harness />);
  drag();
  view.rerender(<Harness items={projects.slice(0, 2)} />);
  expect(document.querySelector(".project-drag-preview")).toBeNull();
  expect(move).not.toHaveBeenCalled();
});

it("scrolls at the sidebar edge and stops the frame loop after cancellation", () => {
  const frames: FrameRequestCallback[] = [];
  vi.stubGlobal(
    "requestAnimationFrame",
    vi.fn((callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    }),
  );
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  render(<Harness />);
  const list = bounds();
  const nav = list.closest<HTMLElement>(".sidebar-navigation")!;
  vi.mocked(nav.getBoundingClientRect).mockReturnValue({
    left: 0,
    right: 240,
    top: 50,
    bottom: 220,
    height: 170,
    width: 240,
  } as DOMRect);
  drag("Gamma", 215);
  act(() => frames.shift()!(16));
  expect(nav.scrollTop).toBe(8);
  fireEvent.keyDown(window, { key: "Escape" });
  act(() => frames.shift()!(32));
  expect(nav.scrollTop).toBe(8);
  expect(move).not.toHaveBeenCalled();
});

it("ignores unpressed mouse motion during an accessibility click instead of starting a drag", () => {
  render(<Harness />);
  bounds();
  const row = screen.getByRole("button", { name: "Alpha" });
  fireEvent.pointerDown(row, { button: 0, clientX: 80, clientY: 110 });
  fireEvent.pointerMove(window, { buttons: 0, clientX: 200, clientY: 210 });
  fireEvent.pointerUp(window, { clientX: 80, clientY: 110 });
  fireEvent.click(row);
  expect(row.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector(".project-drag-preview")).toBeNull();
  expect(move).not.toHaveBeenCalled();
});

it("leaves touch gestures available for scrolling instead of starting a project drag", () => {
  render(<Harness />);
  bounds();
  const row = screen.getByRole("button", { name: "Gamma" });
  const event = new MouseEvent("pointerdown", {
    bubbles: true,
    button: 0,
    clientX: 80,
    clientY: 200,
  });
  Object.defineProperty(event, "pointerType", { value: "touch" });
  fireEvent(row, event);
  fireEvent.pointerMove(window, { buttons: 1, clientX: 80, clientY: 105 });
  fireEvent.pointerUp(window, { clientX: 80, clientY: 105 });
  expect(move).not.toHaveBeenCalled();
  expect(screen.queryByTestId("project-drag-preview")).toBeNull();
});
