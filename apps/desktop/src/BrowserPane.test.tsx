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
import { BrowserPane } from "./BrowserPane";
import { SiteIcon } from "./SiteIcon";
import { readWorkspace, updateWorkspace } from "./workspaceState";
import { appClose } from "./appLifecycle";
import { readBrowserHistory, clearBrowserHistory } from "./browserHistory";
const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  preview: vi.fn(),
  release: vi.fn(),
  events: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock("./api", () => ({ native: true, message: (e: unknown) => String(e) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("./browserPreview", () => ({ browserPreview: mocks.preview }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (name: string, callback: (event: { payload: unknown }) => void) => {
      mocks.events.set(name, callback);
      return () => mocks.events.delete(name);
    },
  ),
}));
const props = {
  workspaceId: "",
  tab: {
    id: "",
    kind: "browser" as const,
    url: "https://example.com",
    title: "Example",
  },
  active: true,
};
beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  props.workspaceId = crypto.randomUUID();
  props.tab.id = crypto.randomUUID();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.preview.mockResolvedValue({
    url: "blob:page-preview",
    release: mocks.release,
  });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    x: 10,
    y: 88,
    width: 320,
    height: 500,
    left: 10,
    top: 88,
    right: 330,
    bottom: 588,
    toJSON() {},
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
it.each([false, true])(
  "fences icon metadata across title changes and navigation (navigated=%s)",
  async (navigated) => {
    let finish!: (value: unknown) => void;
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_site_info"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : undefined,
    );
    const url = `https://metadata-${navigated}.example/doc`;
    const tab = { ...props.tab, url };
    const view = render(
      <>
        <BrowserPane {...props} tab={tab} />
        <SiteIcon url={url} />
      </>,
    );
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        "browser_create",
        expect.anything(),
      ),
    );
    const id = mocks.invoke.mock.calls.find(
      ([command]) => command === "browser_create",
    )![1].id;
    await act(async () =>
      mocks.events.get("browser-location")!({
        payload: { id, url, record: true },
      }),
    );
    await act(async () =>
      mocks.events.get("browser-location")!({
        payload: {
          id,
          url: navigated ? "https://next.example/" : url,
          record: false,
          title: "Updated title",
        },
      }),
    );
    const icon = "data:image/png;base64,aWNvbg==";
    await act(async () =>
      finish({ url, title: "Page", connection: "secure", icon }),
    );
    const image = view.container.querySelector(".site-icon img")!;
    expect(image.getAttribute("src")).toBe(
      navigated ? `${new URL(url).origin}/favicon.ico` : icon,
    );
  },
);
it("submits a bare domain as an HTTPS navigation from the address field", async () => {
  render(<BrowserPane {...props} />);
  const address = screen.getByRole("textbox", { name: "网页地址" });
  await waitFor(() =>
    expect((address as HTMLInputElement).disabled).toBe(false),
  );
  fireEvent.change(address, { target: { value: "www.baidu.com" } });
  fireEvent.submit(address.closest("form")!);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_action",
      expect.objectContaining({
        action: "navigate",
        url: "https://www.baidu.com/",
      }),
    ),
  );
  expect((address as HTMLInputElement).value).toBe("https://www.baidu.com/");
});
it("retains decoded page pixels behind site information and restores the live page after Escape", async () => {
  mocks.invoke.mockImplementation(async (command: string) =>
    command === "browser_site_info"
      ? { url: props.tab.url, title: "Example", connection: "secure" }
      : undefined,
  );
  render(<BrowserPane {...props} />);
  const trigger = screen.getByRole("button", { name: "站点信息" });
  await waitFor(() =>
    expect((trigger as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(trigger);
  expect(await screen.findByText("连接是安全的")).toBeTruthy();
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_sync",
      expect.objectContaining({ bounds: null }),
    ),
  );
  expect(
    document.querySelector(".browser-page-preview")?.getAttribute("src"),
  ).toBe("blob:page-preview");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(document.activeElement).toBe(trigger);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_sync",
      expect.objectContaining({
        bounds: expect.objectContaining({ width: 320 }),
      }),
    ),
  );
  await waitFor(() =>
    expect(document.querySelector(".browser-page-preview")).toBeNull(),
  );
  expect(mocks.release).toHaveBeenCalledOnce();
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_action",
    expect.objectContaining({ action: "navigate" }),
  );
});
it("keeps the page painted while the browser menu is open", async () => {
  render(<BrowserPane {...props} />);
  const trigger = screen.getByRole("button", { name: "浏览器菜单" });
  await waitFor(() =>
    expect((trigger as HTMLButtonElement).disabled).toBe(false),
  );
  mocks.invoke.mockImplementation(
    async (command: string, args: { bounds?: unknown }) => {
      if (command === "browser_sync" && args.bounds === null)
        expect(document.querySelector(".browser-page-preview")).not.toBeNull();
    },
  );
  fireEvent.click(trigger);
  await screen.findByRole("menu");
  await waitFor(() =>
    expect(document.querySelector(".browser-page-preview")).not.toBeNull(),
  );
  expect(mocks.preview).toHaveBeenCalledOnce();
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(mocks.release).toHaveBeenCalledOnce());
});
it("drops a late preview after the menu closes without hiding the page", async () => {
  let finish!: (value: unknown) => void;
  mocks.preview.mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  render(<BrowserPane {...props} />);
  const trigger = screen.getByRole("button", { name: "浏览器菜单" });
  await waitFor(() =>
    expect((trigger as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(trigger);
  await waitFor(() => expect(mocks.preview).toHaveBeenCalledOnce());
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  // Allow the geometry observer to observe the closed menu before capture returns.
  await act(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  mocks.invoke.mockClear();
  await act(async () => finish({ url: "blob:late", release: mocks.release }));
  expect(mocks.release).toHaveBeenCalledOnce();
  expect(document.querySelector(".browser-page-preview")).toBeNull();
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_sync",
    expect.objectContaining({ bounds: null }),
  );
});
it("keeps the live page and closes browser overlays if capture fails", async () => {
  mocks.preview.mockRejectedValue(new Error("网页画面暂不可用"));
  render(<BrowserPane {...props} />);
  const trigger = screen.getByRole("button", { name: "浏览器菜单" });
  await waitFor(() =>
    expect((trigger as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(trigger);
  await waitFor(() => expect(mocks.preview).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_sync",
    expect.objectContaining({ bounds: null }),
  );
  expect(document.querySelector(".browser-page-preview")).toBeNull();
});
it("does not hide or capture the page for a menu outside its bounds", async () => {
  render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_sync",
      expect.objectContaining({ bounds: expect.anything() }),
    ),
  );
  const menu = document.createElement("div");
  menu.setAttribute("role", "menu");
  menu.getBoundingClientRect = () => ({
    x: 500,
    y: 88,
    width: 80,
    height: 100,
    left: 500,
    top: 88,
    right: 580,
    bottom: 188,
    toJSON() {},
  });
  document.body.append(menu);
  await act(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  expect(mocks.preview).not.toHaveBeenCalled();
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_sync",
    expect.objectContaining({ bounds: null }),
  );
  menu.remove();
});
it.each(["hide", "navigate", "unmount"])(
  "releases pending preview pixels after %s",
  async (change) => {
    let finish!: (value: unknown) => void;
    mocks.preview.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const view = render(<BrowserPane {...props} />);
    const trigger = screen.getByRole("button", { name: "浏览器菜单" });
    await waitFor(() =>
      expect((trigger as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(trigger);
    await waitFor(() => expect(mocks.preview).toHaveBeenCalledOnce());
    if (change === "unmount") view.unmount();
    else
      view.rerender(
        <BrowserPane
          {...props}
          active={change !== "hide"}
          tab={
            change === "navigate"
              ? { ...props.tab, url: "https://next.example/" }
              : props.tab
          }
        />,
      );
    await act(async () =>
      finish({ url: "blob:stale", release: mocks.release }),
    );
    expect(mocks.release).toHaveBeenCalledOnce();
    expect(document.querySelector(".browser-page-preview")).toBeNull();
  },
);
it("find submissions do not navigate through the address form across the portal", async () => {
  mocks.invoke.mockImplementation(async (command: string) =>
    command === "browser_find" ? true : undefined,
  );
  render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(
      (screen.getByRole("textbox", { name: "网页地址" }) as HTMLInputElement)
        .disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "浏览器菜单" }));
  fireEvent.click(screen.getByRole("menuitem", { name: /在页面中查找/ }));
  const search = screen.getByRole("textbox", { name: "在页面中查找" });
  fireEvent.change(search, { target: { value: "needle" } });
  await act(async () => fireEvent.submit(search.closest("form")!));
  await act(async () =>
    fireEvent.click(screen.getByRole("button", { name: "下一个匹配" })),
  );
  await act(async () =>
    fireEvent.keyDown(search, { key: "Enter", shiftKey: true }),
  );
  expect(
    mocks.invoke.mock.calls
      .filter(([command]) => command === "browser_find")
      .map(([, args]) => args.backwards),
  ).toEqual([false, false, true]);
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_action",
    expect.objectContaining({ action: "navigate" }),
  );
});
it("creates lazily, hides on tab changes and overlays, and closes hidden surfaces on Quit", async () => {
  const view = render(<BrowserPane {...props} active={false} />);
  expect(mocks.invoke).not.toHaveBeenCalledWith(
    "browser_create",
    expect.anything(),
  );
  view.rerender(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_sync",
      expect.objectContaining({
        bounds: expect.objectContaining({ width: 320 }),
      }),
    ),
  );
  const dialog = document.createElement("div");
  dialog.setAttribute("role", "dialog");
  document.body.append(dialog);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_sync",
      expect.objectContaining({ bounds: null }),
    ),
  );
  dialog.remove();
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_sync",
      expect.objectContaining({
        bounds: expect.objectContaining({ width: 320 }),
      }),
    ),
  );
  view.rerender(<BrowserPane {...props} active={false} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_sync",
      expect.objectContaining({ bounds: null }),
    ),
  );
  await act(() => appClose.run());
  expect(mocks.invoke).toHaveBeenLastCalledWith(
    "browser_close",
    expect.anything(),
  );
});
it("closes after a late native creation and refuses active schemes in the address field", async () => {
  let resolve!: () => void;
  const pending = new Promise<void>((done) => {
    resolve = done;
  });
  mocks.invoke.mockImplementation((command: string) =>
    command === "browser_create" ? pending : Promise.resolve(),
  );
  const view = render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_create",
      expect.anything(),
    ),
  );
  view.unmount();
  await act(async () => resolve());
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_close",
      expect.anything(),
    ),
  );
  mocks.invoke.mockResolvedValue(undefined);
  render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(
      (screen.getByRole("textbox", { name: "网页地址" }) as HTMLInputElement)
        .disabled,
    ).toBe(false),
  );
  fireEvent.change(screen.getByRole("textbox", { name: "网页地址" }), {
    target: { value: "javascript:alert(1)" },
  });
  fireEvent.submit(
    screen.getByRole("textbox", { name: "网页地址" }).closest("form")!,
  );
  expect(
    mocks.invoke.mock.calls.filter(([command]) => command === "browser_action"),
  ).toHaveLength(0);
});

it("routes popup events to an independent tab without changing the opener URL", async () => {
  updateWorkspace(props.workspaceId, {
    tabs: [props.tab],
    current: props.tab.id,
  });
  render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_create",
      expect.anything(),
    ),
  );
  const id = mocks.invoke.mock.calls.find(
    ([command]) => command === "browser_create",
  )![1].id;
  act(() =>
    mocks.events.get("browser-new-tab")?.({
      payload: { id, url: props.tab.url },
    }),
  );
  expect(readWorkspace(props.workspaceId).tabs).toHaveLength(2);
  expect(readWorkspace(props.workspaceId).tabs[0]).toEqual(props.tab);
  expect(readWorkspace(props.workspaceId).current).not.toBe(props.tab.id);
});
it("does not recreate cleared history from reload or late title events", async () => {
  render(<BrowserPane {...props} />);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith(
      "browser_create",
      expect.anything(),
    ),
  );
  const id = mocks.invoke.mock.calls.find(
    ([name]) => name === "browser_create",
  )![1].id;
  act(() =>
    mocks.events.get("browser-location")?.({
      payload: { id, url: props.tab.url, record: true },
    }),
  );
  expect(readBrowserHistory()).toHaveLength(1);
  clearBrowserHistory();
  act(() =>
    mocks.events.get("browser-location")?.({
      payload: { id, url: props.tab.url, title: "Late title", record: false },
    }),
  );
  expect(readBrowserHistory()).toHaveLength(0);
  act(() =>
    mocks.events.get("browser-location")?.({
      payload: { id, url: props.tab.url, record: false },
    }),
  );
  expect(readBrowserHistory()).toHaveLength(0);
  act(() =>
    mocks.events.get("browser-location")?.({
      payload: { id, url: props.tab.url, record: true },
    }),
  );
  expect(readBrowserHistory()[0].title).toBe("Late title");
});
