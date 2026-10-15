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
import { AttachmentImage } from "./AttachmentImage";
import type { SourceData } from "./sourceLoader";
const mocks = vi.hoisted(() => ({ load: vi.fn() }));
vi.mock("./sourceLoader", () => ({ loadSource: mocks.load }));
const data: SourceData = {
  bytes: new Uint8Array([137, 80, 78, 71]),
  mime: "image/png",
  size: 4,
  truncated: false,
};
const props = {
  reference: {
    name: "image.png",
    path: "/remote/cache/image.png",
    directory: false,
  },
  hostId: "ssh",
  projectId: "p",
  sessionId: "s",
  connected: true,
  variant: "chip" as const,
};
beforeEach(() => {
  mocks.load.mockReset().mockResolvedValue(data);
  URL.createObjectURL = vi.fn(() => "blob:fixture");
  URL.revokeObjectURL = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

it("opens a composer image on demand through the owning host, then closes and restores focus", async () => {
  render(<AttachmentImage {...props} />);
  const trigger = screen.getByRole("button", { name: "预览图片 image.png" });
  expect(mocks.load).not.toHaveBeenCalled();
  fireEvent.click(trigger);
  expect(await screen.findByRole("img", { name: "image.png" })).toBeTruthy();
  expect(mocks.load).toHaveBeenCalledWith(
    "ssh",
    "p",
    "s",
    props.reference.path,
    expect.any(Function),
  );
  expect(screen.getByRole("dialog").textContent).not.toContain(
    props.reference.path,
  );
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:fixture");
});

it("lazily renders visible thumbnails, shares the bytes with the dialog and releases off-screen images", async () => {
  let visibility!: (entries: { isIntersecting: boolean }[]) => void;
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      constructor(callback: typeof visibility) {
        visibility = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  render(<AttachmentImage {...props} variant="thumbnail" />);
  expect(mocks.load).not.toHaveBeenCalled();
  act(() => visibility([{ isIntersecting: true }]));
  await screen.findByRole("img", { name: "image.png" });
  fireEvent.click(screen.getByRole("button", { name: "预览图片 image.png" }));
  expect(screen.getAllByRole("img", { name: "image.png" })).toHaveLength(2);
  expect(mocks.load).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "关闭图片预览" }));
  act(() => visibility([{ isIntersecting: false }]));
  await waitFor(() => expect(screen.queryByRole("img")).toBeNull());
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:fixture");
});

it("keeps an already loaded thumbnail during disconnect and fences late responses after changing hosts", async () => {
  const view = render(<AttachmentImage {...props} variant="thumbnail" />);
  await screen.findByRole("img", { name: "image.png" });
  view.rerender(
    <AttachmentImage {...props} variant="thumbnail" connected={false} />,
  );
  expect(screen.getByRole("img").getAttribute("src")).toBe("blob:fixture");
  let finish!: (data: SourceData) => void;
  mocks.load.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender(
    <AttachmentImage {...props} variant="thumbnail" hostId="old" />,
  );
  await waitFor(() => expect(mocks.load).toHaveBeenCalledTimes(2));
  view.rerender(
    <AttachmentImage {...props} variant="thumbnail" hostId="new" />,
  );
  await screen.findByRole("img");
  await act(async () => finish(data));
  expect(URL.createObjectURL).toHaveBeenCalledTimes(2);
  expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
});

it.each(["text/plain", "image/svg+xml"])(
  "does not render %s as a bitmap and allows an explicit retry",
  async (mime) => {
    mocks.load.mockResolvedValueOnce({ ...data, mime });
    render(<AttachmentImage {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "预览图片 image.png" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "不能作为图片预览",
    );
    expect(screen.queryByRole("img")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await screen.findByRole("img", { name: "image.png" });
    expect(mocks.load).toHaveBeenCalledTimes(2);
  },
);

it("closes an enlarged preview and revokes its URL when the conversation becomes hidden", async () => {
  const view = render(<AttachmentImage {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "预览图片 image.png" }));
  await screen.findByRole("img");
  view.rerender(<AttachmentImage {...props} active={false} />);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(URL.revokeObjectURL).toHaveBeenCalled();
  view.rerender(<AttachmentImage {...props} />);
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("bounds concurrent image reads and never starts queued work after disposal", async () => {
  const pending: ((data: SourceData) => void)[] = [];
  mocks.load.mockImplementation(
    () => new Promise((resolve) => pending.push(resolve)),
  );
  const view = render(
    <>
      {Array.from({ length: 5 }, (_, index) => (
        <AttachmentImage
          key={index}
          {...props}
          reference={{ ...props.reference, path: `/remote/${index}.png` }}
          variant="thumbnail"
        />
      ))}
    </>,
  );
  await waitFor(() => expect(mocks.load).toHaveBeenCalledTimes(2));
  expect(pending).toHaveLength(2);
  view.unmount();
  await act(async () => {
    pending.forEach((resolve) => resolve(data));
  });
  expect(mocks.load).toHaveBeenCalledTimes(2);
  expect(URL.createObjectURL).not.toHaveBeenCalled();
});
