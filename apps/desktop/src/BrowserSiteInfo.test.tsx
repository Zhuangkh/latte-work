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
import { BrowserSiteInfo } from "./BrowserSiteInfo";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const props = {
  id: "native-tab",
  url: "https://example.com/guide",
  active: true,
  ready: true,
};
beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue({
    url: props.url,
    title: "Example guide",
    connection: "secure",
  });
});
afterEach(cleanup);
it("reads the current native page, closes with Escape and restores the trigger", async () => {
  render(<BrowserSiteInfo {...props} />);
  const trigger = screen.getByRole("button", { name: "站点信息" });
  fireEvent.click(trigger);
  expect(await screen.findByText("连接是安全的")).toBeTruthy();
  expect(screen.getByText("example.com")).toBeTruthy();
  expect(screen.getByText("Example guide")).toBeTruthy();
  expect(invoke).toHaveBeenCalledExactlyOnceWith("browser_site_info", {
    id: "native-tab",
    icon: false,
  });
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "站点信息" })).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
it("does not infer trust from an HTTPS address or accept an old page snapshot", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(<BrowserSiteInfo {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "站点信息" }));
  expect(screen.queryByText("连接是安全的")).toBeNull();
  await act(async () =>
    finish({ url: "https://old.example/", connection: "secure" }),
  );
  expect(screen.getByText("页面已变化，请重新查看站点信息")).toBeTruthy();
  expect(screen.queryByText("连接是安全的")).toBeNull();
});
it("shows actual HTTP status, closes when hidden, and keeps blank tabs disabled", async () => {
  invoke.mockResolvedValue({
    url: "http://example.com/",
    title: "Example",
    connection: "insecure",
  });
  const view = render(<BrowserSiteInfo {...props} url="http://example.com/" />);
  fireEvent.click(screen.getByRole("button", { name: "站点信息" }));
  expect(await screen.findByText("连接未加密")).toBeTruthy();
  view.rerender(<BrowserSiteInfo {...props} active={false} />);
  expect(screen.queryByRole("dialog", { name: "站点信息" })).toBeNull();
  view.rerender(<BrowserSiteInfo {...props} url="" />);
  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "站点信息" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true),
  );
});
it("closes on outside click and does not expose fictitious permission settings", async () => {
  render(<BrowserSiteInfo {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "站点信息" }));
  await screen.findByText("连接是安全的");
  expect(screen.queryByText("网站设置")).toBeNull();
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("dialog", { name: "站点信息" })).toBeNull();
});
it("shows real connection details without copy or external-browser actions", async () => {
  invoke.mockResolvedValue({
    url: props.url,
    title: "Example guide",
    connection: "secure",
    certificate: { subject: "example.com", issuer: "Example CA" },
  });
  render(<BrowserSiteInfo {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "站点信息" }));
  fireEvent.click(await screen.findByRole("button", { name: "连接是安全的" }));
  expect(screen.getByText("Example CA")).toBeTruthy();
  expect(screen.getByText("HTTPS")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "复制网址" })).toBeNull();
  expect(
    screen.queryByRole("button", { name: "在系统浏览器中打开" }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "返回站点信息" }));
  expect(
    screen.getByRole("button", { name: "Cookie 和网站数据" }),
  ).toBeTruthy();
});
async function openData() {
  render(<BrowserSiteInfo {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "站点信息" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Cookie 和网站数据" }),
  );
}
const siteData = {
  url: props.url,
  token: "one-use",
  domains: ["example.com"],
  types: [
    "WKWebsiteDataTypeCookies",
    "WKWebsiteDataTypeLocalStorage",
    "WKWebsiteDataTypeDiskCache",
  ],
};
it("only clears the displayed site records after confirmation, and prevents double submission", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(
    async (command: string, args: { token?: string }) =>
      command === "browser_site_info"
        ? { url: props.url, connection: "secure" }
        : args.token
          ? new Promise((resolve) => {
              finish = resolve;
            })
          : siteData,
  );
  await openData();
  fireEvent.click(await screen.findByRole("button", { name: "清除网站数据…" }));
  expect(screen.getByRole("dialog", { name: "清除网站数据" })).toBeTruthy();
  expect(invoke).not.toHaveBeenCalledWith(
    "browser_site_data",
    expect.objectContaining({ token: "one-use" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(screen.getByRole("dialog", { name: "站点信息" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "清除网站数据…" }));
  const confirm = screen.getByRole("button", { name: "清除并刷新" });
  fireEvent.click(confirm);
  fireEvent.click(confirm);
  expect(
    invoke.mock.calls.filter(
      ([command, args]) => command === "browser_site_data" && args.token,
    ),
  ).toHaveLength(1);
  expect(invoke).toHaveBeenCalledWith("browser_site_data", {
    id: props.id,
    url: props.url,
    token: "one-use",
  });
  await act(async () => finish({ cleared: true }));
  expect(screen.queryByRole("dialog")).toBeNull();
});
it("requires another read after a failed clear instead of replaying the grant", async () => {
  invoke.mockImplementation(
    async (command: string, args: { token?: string }) => {
      if (command === "browser_site_info")
        return { url: props.url, connection: "secure" };
      if (args.token) throw new Error("确认已失效");
      return siteData;
    },
  );
  await openData();
  fireEvent.click(await screen.findByRole("button", { name: "清除网站数据…" }));
  fireEvent.click(screen.getByRole("button", { name: "清除并刷新" }));
  expect(await screen.findByText("确认已失效")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "清除网站数据…" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "重新读取" }));
  expect(
    await screen.findByRole("button", { name: "清除网站数据…" }),
  ).toBeTruthy();
});
it("ignores stale data responses and never offers a clear action for an empty store", async () => {
  let finish!: (value: unknown) => void;
  invoke.mockImplementation(async (command: string) =>
    command === "browser_site_info"
      ? { url: props.url, connection: "secure" }
      : new Promise((resolve) => {
          finish = resolve;
        }),
  );
  await openData();
  fireEvent.click(screen.getByRole("button", { name: "返回站点信息" }));
  await act(async () => finish(siteData));
  expect(screen.queryByRole("button", { name: "清除网站数据…" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Cookie 和网站数据" }));
  await act(async () => finish({ ...siteData, domains: [], types: [] }));
  expect(screen.getByText("此站点暂未保存网站数据")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "清除网站数据…" })).toBeNull();
});
