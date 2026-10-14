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
import { BrowserTools } from "./BrowserTools";
import { readBrowserHistory, recordBrowserVisit } from "./browserHistory";
const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  events: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock("./api", () => ({
  native: true,
  message: (error: unknown) => String(error),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (name: string, callback: (event: { payload: unknown }) => void) => {
      mocks.events.set(name, callback);
      return () => mocks.events.delete(name);
    },
  ),
}));
const report = vi.fn();
const props = {
  id: crypto.randomUUID(),
  workspaceId: crypto.randomUUID(),
  active: true,
  ready: true,
  url: "https://example.com",
  reportError: report,
  enqueue: (work: () => Promise<unknown>) => work(),
};
beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    width: 320,
    height: 500,
    left: 0,
    top: 0,
    right: 320,
    bottom: 500,
    toJSON() {},
  });
  mocks.invoke.mockImplementation(async (name: string) =>
    name === "browser_downloads"
      ? []
      : name === "browser_find"
        ? true
        : undefined,
  );
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
function mount(overrides: Partial<typeof props> = {}) {
  return render(
    <div className="browser-pane">
      <div className="browser-toolbar">
        <BrowserTools {...props} {...overrides} />
      </div>
    </div>,
  );
}
function menu() {
  fireEvent.click(screen.getByRole("button", { name: "浏览器菜单" }));
}
function openFind() {
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: /在页面中查找/ }));
  const input = screen.getByRole("textbox", { name: "在页面中查找" });
  fireEvent.change(input, { target: { value: "needle" } });
  return input;
}
it.each(["close", "Escape"])(
  "stops native Find on %s and ignores its late result",
  async (action) => {
    let complete: (found: boolean) => void = () => {};
    const pending = new Promise<boolean>((resolve) => {
      complete = resolve;
    });
    mocks.invoke.mockImplementation(
      (name: string, args?: { query?: string }) => {
        if (name === "browser_find" && args?.query) return pending;
        return Promise.resolve(name === "browser_downloads" ? [] : false);
      },
    );
    mount();
    const input = openFind();
    fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
        id: props.id,
        query: "needle",
        backwards: false,
      }),
    );
    if (action === "Escape") fireEvent.keyDown(input, { key: "Escape" });
    else fireEvent.click(screen.getByRole("button", { name: "关闭查找" }));
    expect(screen.queryByRole("textbox", { name: "在页面中查找" })).toBeNull();
    expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
      id: props.id,
      query: "",
      backwards: false,
    });
    await act(async () => complete(true));
    menu();
    fireEvent.click(screen.getByRole("menuitem", { name: /在页面中查找/ }));
    expect(screen.getByRole("status").textContent).toBe("");
    expect(
      mocks.invoke.mock.calls.filter(
        ([name, args]) => name === "browser_find" && args.query,
      ),
    ).toHaveLength(1);
  },
);
it("does not start queued Find after close", async () => {
  let queued: (() => Promise<unknown>) | undefined;
  mount({
    enqueue: (work) => {
      queued = work;
      return Promise.resolve(null);
    },
  });
  openFind();
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  fireEvent.click(screen.getByRole("button", { name: "关闭查找" }));
  await act(async () => {
    await queued?.();
  });
  expect(
    mocks.invoke.mock.calls.some(
      ([name, args]) => name === "browser_find" && args.query === "needle",
    ),
  ).toBe(false);
});
it("preserves a Stop failure when the cancelled Find later succeeds", async () => {
  let complete: (found: boolean) => void = () => {};
  const pending = new Promise<boolean>((resolve) => {
    complete = resolve;
  });
  mocks.invoke.mockImplementation((name: string, args?: { query?: string }) => {
    if (name === "browser_find")
      return args?.query ? pending : Promise.reject(new Error("Stop failed"));
    return Promise.resolve([]);
  });
  mount();
  openFind();
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
      id: props.id,
      query: "needle",
      backwards: false,
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: "关闭查找" }));
  await waitFor(() =>
    expect(report).toHaveBeenLastCalledWith("Error: Stop failed"),
  );
  await act(async () => complete(true));
  expect(report).toHaveBeenLastCalledWith("Error: Stop failed");
});
it("waits for pending Stop before starting a newly opened Find", async () => {
  let stopped: () => void = () => {};
  const pending = new Promise<void>((resolve) => {
    stopped = resolve;
  });
  mocks.invoke.mockImplementation((name: string, args?: { query?: string }) => {
    if (name === "browser_find")
      return args?.query ? Promise.resolve(true) : pending;
    return Promise.resolve([]);
  });
  mount();
  openFind();
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("已找到"),
  );
  fireEvent.click(screen.getByRole("button", { name: "关闭查找" }));
  openFind();
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  await act(async () => {});
  expect(
    mocks.invoke.mock.calls.filter(
      ([name, args]) => name === "browser_find" && args.query,
    ),
  ).toHaveLength(1);
  await act(async () => stopped());
  await waitFor(() =>
    expect(
      mocks.invoke.mock.calls.filter(
        ([name, args]) => name === "browser_find" && args.query,
      ),
    ).toHaveLength(2),
  );
});
it("clears empty queries and stops active Find on unmount", async () => {
  const mounted = mount();
  const input = openFind();
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("已找到"),
  );
  fireEvent.change(input, { target: { value: "" } });
  expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
    id: props.id,
    query: "",
    backwards: false,
  });
  expect(screen.getByRole("status").textContent).toBe("");
  fireEvent.change(input, { target: { value: "again" } });
  fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
      id: props.id,
      query: "again",
      backwards: true,
    }),
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("已找到"),
  );
  mocks.invoke.mockClear();
  mounted.unmount();
  expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
    id: props.id,
    query: "",
    backwards: false,
  });
});
it.each(["navigation", "hide", "not-ready"])(
  "resets Find on %s",
  async (transition) => {
    const mounted = mount();
    openFind();
    fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("已找到"),
    );
    mocks.invoke.mockClear();
    const next = {
      ...props,
      url: transition === "navigation" ? "https://other.example" : props.url,
      active: transition !== "hide",
      ready: transition !== "not-ready",
    };
    mounted.rerender(
      <div className="browser-pane">
        <div className="browser-toolbar">
          <BrowserTools {...next} />
        </div>
      </div>,
    );
    expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
      id: props.id,
      query: "",
      backwards: false,
    });
    expect(
      mocks.invoke.mock.calls.some(
        ([name, args]) => name === "browser_find" && args.query === "needle",
      ),
    ).toBe(false);
  },
);
it("runs find and zoom on the selected native view and routes keyboard shortcuts", async () => {
  mount();
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: /在页面中查找/ }));
  fireEvent.change(screen.getByRole("textbox", { name: "在页面中查找" }), {
    target: { value: "needle" },
  });
  fireEvent.click(screen.getByRole("button", { name: "下一个匹配" }));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_find", {
      id: props.id,
      query: "needle",
      backwards: false,
    }),
  );
  expect(screen.getByRole("status").textContent).toBe("已找到");
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: "放大网页" }));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_action", {
      id: props.id,
      action: "zoom",
      zoom: 1.1,
    }),
  );
  expect(
    screen.getByRole("menuitem", { name: "重置网页缩放" }).textContent,
  ).toBe("110%");
  fireEvent.keyDown(window, { key: "Escape" });
  await act(async () =>
    mocks.events.get("browser-shortcut")?.({ payload: "zoom-reset" }),
  );
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_action", {
      id: props.id,
      action: "zoom",
      zoom: 1,
    }),
  );
});
it("requires explicit confirmation to clear data and preserves app settings", async () => {
  recordBrowserVisit("https://example.com", "Example");
  localStorage.setItem("latte-work.link-target.v1", "system");
  mount();
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: "清除浏览数据…" }));
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_clear_data",
    expect.anything(),
  );
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(readBrowserHistory()).toHaveLength(1);
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: "清除浏览数据…" }));
  fireEvent.click(screen.getByRole("button", { name: "清除" }));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("browser_clear_data", {
      id: props.id,
    }),
  );
  await waitFor(() => expect(readBrowserHistory()).toHaveLength(0));
  expect(localStorage.getItem("latte-work.link-target.v1")).toBe("system");
});
it("keeps history after a failed native clear and shows real download outcomes", async () => {
  recordBrowserVisit("https://example.com", "Example");
  mocks.invoke.mockImplementation(async (name: string) => {
    if (name === "browser_clear_data") throw new Error("failed");
    return name === "browser_downloads" ? [] : undefined;
  });
  mount();
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: "清除浏览数据…" }));
  fireEvent.click(screen.getByRole("button", { name: "清除" }));
  await waitFor(() => expect(report).toHaveBeenCalledWith("Error: failed"));
  expect(readBrowserHistory()).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  await act(async () =>
    mocks.events.get("browser-downloads")?.({
      payload: [
        {
          id: "one",
          name: "a.pdf",
          url: "https://example.com/a.pdf",
          status: "interrupted",
          error: "应用退出时下载尚未完成",
        },
      ],
    }),
  );
  menu();
  fireEvent.click(screen.getByRole("menuitem", { name: "下载" }));
  expect(screen.getByText("应用退出时下载尚未完成")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "显示位置" })).toBeNull();
});
