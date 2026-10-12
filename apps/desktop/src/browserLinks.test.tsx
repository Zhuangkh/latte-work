// @vitest-environment jsdom
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { BrowserSettings } from "./BrowserSettings";
import { LinkHandling } from "./LinkHandling";
import { MessageContent } from "./MessageContent";
import { openBrowser, readWorkspace } from "./workspaceState";
import {
  browserAddress,
  readLinkTarget,
  saveLinkTarget,
  webLink,
} from "./browserLinks";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("./api", () => ({ native: true, message: (e: unknown) => String(e) }));
beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  mocks.invoke.mockResolvedValue(undefined);
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);
it("normalizes address-bar domains without loosening link validation", () => {
  for (const [input, expected] of [
    [" www.baidu.com ", "https://www.baidu.com/"],
    ["example.com/docs?q=one#two", "https://example.com/docs?q=one#two"],
    ["localhost:8080/page", "https://localhost:8080/page"],
    ["127.0.0.1:8080", "https://127.0.0.1:8080/"],
    ["[::1]:8080", "https://[::1]:8080/"],
    ["http://example.com", "http://example.com/"],
  ])
    expect(browserAddress(input)).toBe(expected);
  expect(webLink("www.baidu.com")).toBeNull();
  for (const input of [
    "",
    "javascript:alert(1)",
    "data:text/html,x",
    "file:///tmp/a",
    "ftp://example.com",
    "https://u:p@example.com",
    "tauri.localhost",
    "ipc.localhost",
  ])
    expect(browserAddress(input)).toBeNull();
});
it("routes Markdown and bare URLs through the saved default and explicit context menu", async () => {
  const workspaceId = crypto.randomUUID();
  render(
    <LinkHandling workspaceId={workspaceId}>
      <MessageContent text="[文档](https://example.com/docs) https://example.org" />
    </LinkHandling>,
  );
  fireEvent.click(screen.getByRole("link", { name: "文档" }));
  expect(readWorkspace(workspaceId).tabs).toMatchObject([
    { kind: "browser", url: "https://example.com/docs" },
  ]);
  saveLinkTarget("system");
  fireEvent.click(screen.getByRole("link", { name: "文档" }));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("open_external_url", {
      url: "https://example.com/docs",
    }),
  );
  fireEvent.contextMenu(
    screen.getByRole("link", { name: "https://example.org" }),
    { clientX: 10, clientY: 10 },
  );
  expect(screen.getByRole("menuitem", { name: "在内置新标签页中打开" })).toBe(
    document.activeElement,
  );
  fireEvent.click(
    screen.getByRole("menuitem", { name: "在内置新标签页中打开" }),
  );
  expect(readWorkspace(workspaceId).tabs).toHaveLength(2);
  fireEvent.keyDown(screen.getByRole("link", { name: "文档" }), {
    key: "F10",
    shiftKey: true,
  });
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(
    screen.getByRole("link", { name: "文档" }),
  );
});
it("saves browser preferences immediately and retains the previous value on a write failure", () => {
  render(<BrowserSettings />);
  fireEvent.click(screen.getByRole("combobox", { name: "链接默认打开方式" }));
  fireEvent.click(screen.getByRole("option", { name: /系统默认浏览器/ }));
  expect(readLinkTarget()).toBe("system");
  const store = vi
    .spyOn(Storage.prototype, "setItem")
    .mockImplementation(() => {
      throw new Error("full");
    });
  fireEvent.click(screen.getByRole("combobox", { name: "链接默认打开方式" }));
  fireEvent.click(screen.getByRole("option", { name: /内置浏览器/ }));
  expect(screen.getByRole("alert").textContent).toContain("设置保存失败");
  expect(readLinkTarget()).toBe("system");
  store.mockRestore();
});
it("rejects active and credential URLs and restores only valid browser tabs", async () => {
  for (const value of [
    "javascript:alert(1)",
    "file:///tmp/a",
    "data:text/html,x",
    "https://u:p@example.com",
    "https://tauri.localhost",
  ])
    expect(webLink(value)).toBeNull();
  const key = crypto.randomUUID();
  const id = crypto.randomUUID();
  openBrowser(key, "https://example.com");
  openBrowser(key, "https://example.com/");
  expect(readWorkspace(key).tabs).toHaveLength(1);
  localStorage.setItem(
    "latte-work.conversation-workspace.v1:" + id,
    JSON.stringify({
      tabs: [
        {
          id: crypto.randomUUID(),
          kind: "browser",
          url: "javascript:alert(1)",
          title: "bad",
        },
        {
          id: crypto.randomUUID(),
          kind: "browser",
          url: "https://example.com",
          title: "Example",
        },
      ],
    }),
  );
  vi.resetModules();
  const restored = await import("./workspaceState");
  expect(restored.readWorkspace(id).tabs).toMatchObject([
    { url: "https://example.com", title: "Example" },
  ]);
});
