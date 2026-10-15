// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { formatMessageTimestamp, MessageTimestamp } from "./MessageTimestamp";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
const local = (
  year: number,
  month: number,
  day: number,
  hour = 15,
  minute = 52,
) => new Date(year, month - 1, day, hour, minute).getTime();

it.each([
  [local(2026, 10, 13), "今天 15:52"],
  [local(2026, 10, 12), "昨天 15:52"],
  [local(2026, 10, 11), "前天 15:52"],
  [local(2026, 10, 10), "10月10日 15:52"],
  [local(2025, 10, 13), "2025年10月13日 15:52"],
  [local(2026, 10, 14), "10月14日 15:52"],
  [local(2027, 1, 1), "2027年1月1日 15:52"],
])("formats %s with a useful calendar date", (at, expected) => {
  expect(formatMessageTimestamp(at, local(2026, 10, 13, 18))).toBe(expected);
});

it("uses calendar days across midnight, month/year boundaries and daylight savings", () => {
  expect(
    formatMessageTimestamp(
      local(2025, 12, 31, 23, 59),
      local(2026, 1, 1, 0, 1),
    ),
  ).toBe("昨天 23:59");
  expect(formatMessageTimestamp(local(2025, 12, 30), local(2026, 1, 1))).toBe(
    "前天 15:52",
  );
  expect(formatMessageTimestamp(local(2025, 12, 29), local(2026, 1, 1))).toBe(
    "2025年12月29日 15:52",
  );
  expect(formatMessageTimestamp(local(2026, 2, 28), local(2026, 3, 1))).toBe(
    "昨天 15:52",
  );
  expect(
    formatMessageTimestamp(local(2026, 3, 8, 0, 30), local(2026, 3, 9, 0, 1)),
  ).toBe("昨天 00:30");
});

it("updates idle timestamps at midnight without changing their original time", () => {
  vi.useFakeTimers();
  vi.setSystemTime(local(2026, 10, 13, 23, 59) + 59_000);
  const at = local(2026, 10, 13);
  const view = render(<MessageTimestamp at={at} label="回复时间" />);
  const time = view.container.querySelector("time")!;
  expect(time.textContent).toBe("今天 15:52");
  act(() => vi.advanceTimersByTime(1002));
  expect(time.textContent).toBe("昨天 15:52");
  expect(time.dateTime).toBe(new Date(at).toISOString());
  expect(time.title).toContain("2026");
  act(() => {
    vi.setSystemTime(local(2026, 10, 15));
    window.dispatchEvent(new Event("focus"));
  });
  expect(time.textContent).toBe("前天 15:52");
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it("omits invalid or absent times", () => {
  expect(formatMessageTimestamp(NaN)).toBeNull();
  expect(formatMessageTimestamp(Date.now(), NaN)).toBeNull();
  const view = render(<MessageTimestamp label="回复时间" />);
  expect(view.container.querySelector("time")).toBeNull();
});
