// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { ReviewWorkspace } from "./ReviewWorkspace";
import type { GitReviewFile } from "./protocol";
let observers: { callback: IntersectionObserverCallback; element: Element }[];
beforeEach(() => {
  observers = [];
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      callback: IntersectionObserverCallback;
      constructor(callback: IntersectionObserverCallback) {
        this.callback = callback;
      }
      observe(element: Element) {
        observers.push({ callback: this.callback, element });
      }
      disconnect() {}
    },
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
const file = (path: string, status = "M"): GitReviewFile => ({
  path,
  status,
  added: 1,
  removed: 1,
  previous_path: null,
  binary: false,
  untracked: false,
});
const defaults = {
  scopeControl: "本轮变更",
  files: [
    file("crates/server/main.rs"),
    file("crates/protocol/lib.rs"),
    file("assets/icon.png", "A"),
  ],
  added: 3,
  removed: 3,
  loading: false,
  active: true,
  boundary: "已保存的本轮快照",
  refresh: vi.fn(),
};
const patch = (path: string) => ({
  text: `--- a/${path}\n+++ b/${path}\n@@ -526 +526 @@\n-old\n+new`,
  truncated: false,
});
it("auto-opens real diffs, navigates grouped folders, preserves layout cache and reopens a collapsed target", async () => {
  const load = vi.fn(async (path: string) => patch(path));
  render(<ReviewWorkspace {...defaults} loadDiff={load} />);
  await screen.findByRole("region", { name: "统一文件差异" });
  expect(load).toHaveBeenCalledTimes(1);
  const nav = screen.getByRole("navigation", { name: "变更目录" });
  fireEvent.click(within(nav).getByRole("button", { name: /lib.rs/ }));
  await waitFor(() =>
    expect(load).toHaveBeenCalledWith("crates/protocol/lib.rs"),
  );
  expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();
  fireEvent.scroll(screen.getByLabelText("文件差异浏览"), {
    target: { scrollTop: 1 },
  });
  expect(
    within(nav)
      .getByRole("button", { name: /lib.rs/ })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "并排差异" }));
  expect(screen.getAllByRole("region", { name: "并排文件差异" })).toHaveLength(
    2,
  );
  expect(load).toHaveBeenCalledTimes(2);
  fireEvent.click(
    screen.getByRole("button", { name: "折叠 crates/protocol/lib.rs" }),
  );
  expect(screen.getAllByRole("region", { name: "并排文件差异" })).toHaveLength(
    1,
  );
  fireEvent.click(within(nav).getByRole("button", { name: /lib.rs/ }));
  expect(screen.getAllByRole("region", { name: "并排文件差异" })).toHaveLength(
    2,
  );
  expect(load).toHaveBeenCalledTimes(2);
  fireEvent.change(screen.getByRole("textbox", { name: "筛选文件" }), {
    target: { value: "server" },
  });
  expect(within(nav).queryByRole("button", { name: /lib.rs/ })).toBeNull();
  expect(screen.getAllByRole("region", { name: "并排文件差异" })).toHaveLength(
    2,
  );
});
it("bounds visible-file reads to three and cancels queued work when the panel hides", async () => {
  const finishes: (() => void)[] = [];
  const load = vi.fn(
    (path: string) =>
      new Promise<ReturnType<typeof patch>>((resolve) =>
        finishes.push(() => resolve(patch(path))),
      ),
  );
  const p = {
    ...defaults,
    files: Array.from({ length: 8 }, (_, i) => file(`src/f${i}.ts`)),
    loadDiff: load,
  };
  const view = render(<ReviewWorkspace {...p} />);
  await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
  await act(async () =>
    observers.forEach(({ callback, element }) =>
      callback(
        [
          { isIntersecting: true, target: element },
        ] as IntersectionObserverEntry[],
        {} as IntersectionObserver,
      ),
    ),
  );
  expect(load).toHaveBeenCalledTimes(3);
  view.rerender(<ReviewWorkspace {...p} active={false} />);
  await act(async () => finishes.forEach((finish) => finish()));
  expect(load).toHaveBeenCalledTimes(3);
  expect(screen.queryByRole("region", { name: "统一文件差异" })).toBeNull();
});
it("rejects a late patch after a new loader and reports read failures with a usable retry", async () => {
  let finish!: (data: ReturnType<typeof patch>) => void;
  const old = vi.fn(
    () =>
      new Promise<ReturnType<typeof patch>>((resolve) => {
        finish = resolve;
      }),
  );
  const p = { ...defaults, files: [file("a.txt")] };
  const view = render(<ReviewWorkspace {...p} loadDiff={old} />);
  await waitFor(() => expect(old).toHaveBeenCalled());
  const next = vi.fn(async () => ({
    text: "@@ -1 +1 @@\n+CURRENT",
    truncated: false,
  }));
  view.rerender(<ReviewWorkspace {...p} loadDiff={next} />);
  await screen.findByText("+CURRENT");
  await act(async () =>
    finish({ text: "@@ -1 +1 @@\n+STALE", truncated: false }),
  );
  expect(screen.queryByText("+STALE")).toBeNull();
  const failure = vi
    .fn()
    .mockRejectedValueOnce(new Error("file unavailable"))
    .mockResolvedValue({ text: "@@ -1 +1 @@\n+RECOVERED", truncated: false });
  view.rerender(<ReviewWorkspace {...p} loadDiff={failure} />);
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  await screen.findByText("+RECOVERED");
  expect(failure).toHaveBeenCalledTimes(2);
});

it("restores cached offscreen code without issuing another read", async () => {
  const load = vi.fn(async () => ({
    text: "@@ -1 +1 @@\n+VISIBLE",
    truncated: false,
  }));
  render(
    <ReviewWorkspace {...defaults} files={[file("a.txt")]} loadDiff={load} />,
  );
  await screen.findByText("+VISIBLE");
  const observer = observers[0];
  await act(async () =>
    observer.callback(
      [
        { isIntersecting: false, target: observer.element },
      ] as IntersectionObserverEntry[],
      {} as IntersectionObserver,
    ),
  );
  expect(screen.queryByText("+VISIBLE")).toBeNull();
  await act(async () =>
    observer.callback(
      [
        { isIntersecting: true, target: observer.element },
      ] as IntersectionObserverEntry[],
      {} as IntersectionObserver,
    ),
  );
  expect(screen.getByText("+VISIBLE")).toBeTruthy();
  expect(load).toHaveBeenCalledTimes(1);
});
