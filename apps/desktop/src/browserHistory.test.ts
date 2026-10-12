// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import {
  clearBrowserHistory,
  readBrowserHistory,
  recordBrowserVisit,
  removeBrowserVisit,
  updateBrowserVisitTitle,
} from "./browserHistory";
beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});
it("bounds persisted visits, updates titles, and rejects non-web entries", () => {
  recordBrowserVisit("javascript:alert(1)");
  expect(readBrowserHistory()).toEqual([]);
  recordBrowserVisit("https://example.com", "First");
  recordBrowserVisit("https://example.com/", "Final title");
  expect(readBrowserHistory()).toHaveLength(1);
  expect(readBrowserHistory()[0].title).toBe("Final title");
  for (let i = 0; i < 510; i++) recordBrowserVisit(`https://example.com/${i}`);
  const items = readBrowserHistory();
  expect(items).toHaveLength(500);
  expect(items[0].url).toBe("https://example.com/10");
  removeBrowserVisit(items[0].id);
  expect(readBrowserHistory()).toHaveLength(499);
  clearBrowserHistory();
  updateBrowserVisitTitle("https://example.com/509", "Late page title");
  expect(readBrowserHistory()).toEqual([]);
});
it("retains saved records and unrelated settings when storage writes fail", () => {
  recordBrowserVisit("https://example.com");
  localStorage.setItem("latte-work.link-target.v1", "system");
  const previous = readBrowserHistory();
  const store = vi
    .spyOn(Storage.prototype, "setItem")
    .mockImplementation(() => {
      throw new Error("full");
    });
  expect(() => clearBrowserHistory()).toThrow("full");
  store.mockRestore();
  expect(readBrowserHistory()).toEqual(previous);
  clearBrowserHistory();
  expect(localStorage.getItem("latte-work.link-target.v1")).toBe("system");
});
