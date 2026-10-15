// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { UserMessage } from "./UserMessage";
import { legacyReferenceMarker } from "./referenceMessage";
const mocks = vi.hoisted(() => ({ copyText: vi.fn(), load: vi.fn() }));
vi.mock("./clipboard", () => ({ copyText: mocks.copyText }));
vi.mock("./sourceLoader", () => ({ loadSource: mocks.load }));
afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

it("uses the persisted timestamp and copies the exact original message", async () => {
  mocks.copyText.mockResolvedValue(undefined);
  const at = Date.parse("2026-09-30T09:35:00Z");
  const text = "第一行\n第二行  ";
  const view = render(<UserMessage text={text} at={at} />);
  const time = view.container.querySelector("time")!;
  expect(time.dateTime).toBe("2026-09-30T09:35:00.000Z");
  expect(time.textContent).toMatch(/ \d{2}:35$/);
  expect(time.textContent).not.toContain("星期");
  expect(time.title).toContain("2026");
  fireEvent.click(screen.getByRole("button", { name: "复制消息" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "已复制" })).toBeTruthy(),
  );
  expect(mocks.copyText).toHaveBeenCalledWith(text);
});

it("reports copy failure and does not show a success indicator", async () => {
  mocks.copyText.mockRejectedValue(new Error("Clipboard unavailable"));
  render(<UserMessage text="继续" at={Date.now()} />);
  fireEvent.click(screen.getByRole("button", { name: "复制消息" }));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Clipboard unavailable",
  );
  expect(screen.queryByRole("button", { name: "已复制" })).toBeNull();
});

it("keeps copying available without inventing a timestamp for invalid input", () => {
  const view = render(<UserMessage text="继续" at={NaN} />);
  expect(view.container.querySelector("time")).toBeNull();
  expect(screen.getByRole("button", { name: "复制消息" })).toBeTruthy();
});

it("shows persisted legacy image attachments as thumbnails and keeps paths only in hover hints", async () => {
  URL.createObjectURL = vi.fn(() => "blob:image");
  URL.revokeObjectURL = vi.fn();
  mocks.load.mockResolvedValue({
    bytes: new Uint8Array([137, 80, 78, 71]),
    mime: "image/png",
    size: 4,
    truncated: false,
  });
  mocks.copyText.mockResolvedValue(undefined);
  const path = "/remote/cache/image.png";
  const text = `怎么看这张图？${legacyReferenceMarker}${JSON.stringify([{ name: "image.png", path, type: "file", kind: "image", mime_type: "image/png" }], null, 2)}`;
  const view = render(
    <UserMessage
      text={text}
      at={Date.now()}
      hostId="ssh"
      projectId="p"
      sessionId="s"
      connected
    />,
  );
  await screen.findByRole("img", { name: "image.png" });
  expect(screen.getByRole("button", { name: "预览图片 image.png" }).title).toBe(
    path,
  );
  expect(view.container.textContent).toContain("怎么看这张图？");
  expect(view.container.textContent).not.toContain(path);
  expect(view.container.textContent).not.toContain("mime_type");
  expect(mocks.load).toHaveBeenCalledWith(
    "ssh",
    "p",
    "s",
    path,
    expect.any(Function),
  );
  fireEvent.click(screen.getByRole("button", { name: "复制消息" }));
  await waitFor(() => expect(mocks.copyText).toHaveBeenCalledWith(text));
});
