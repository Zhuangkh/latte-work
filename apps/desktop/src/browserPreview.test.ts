// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { browserPreview } from "./browserPreview";
const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  decode: vi.fn(),
  create: vi.fn(),
  revoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
beforeEach(() => {
  vi.clearAllMocks();
  mocks.invoke.mockResolvedValue(new ArrayBuffer(20));
  mocks.decode.mockResolvedValue(undefined);
  mocks.create.mockReturnValue("blob:page-preview");
  const OriginalURL = URL;
  vi.stubGlobal(
    "URL",
    class extends OriginalURL {
      static createObjectURL = mocks.create;
      static revokeObjectURL = mocks.revoke;
    },
  );
  vi.stubGlobal(
    "Image",
    class {
      src = "";
      decode = mocks.decode;
    },
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
it("decodes bounded binary pixels before handing them to the page and releases the object URL", async () => {
  const picture = await browserPreview("pane-id");
  expect(mocks.invoke).toHaveBeenCalledWith("browser_preview", {
    id: "pane-id",
  });
  expect(mocks.create.mock.calls[0][0].type).toBe("image/jpeg");
  expect(mocks.decode).toHaveBeenCalledOnce();
  expect(mocks.revoke).not.toHaveBeenCalled();
  picture.release();
  expect(mocks.revoke).toHaveBeenCalledWith(picture.url);
});
it.each([new ArrayBuffer(0), new ArrayBuffer(8 * 1024 * 1024 + 1), [1, 2, 3]])(
  "rejects invalid or oversized native payloads",
  async (bytes) => {
    mocks.invoke.mockResolvedValue(bytes);
    await expect(browserPreview("pane")).rejects.toThrow("网页画面暂不可用");
    expect(mocks.create).not.toHaveBeenCalled();
  },
);
it("releases an undecodable image", async () => {
  mocks.decode.mockRejectedValue(new Error("invalid jpeg"));
  await expect(browserPreview("pane")).rejects.toThrow("invalid jpeg");
  expect(mocks.revoke).toHaveBeenCalledOnce();
});
it("bounds image decoding time and releases a stalled image", async () => {
  vi.useFakeTimers();
  mocks.decode.mockReturnValue(new Promise(() => {}));
  const pending = expect(browserPreview("pane")).rejects.toThrow(
    "网页画面读取超时",
  );
  await vi.advanceTimersByTimeAsync(1500);
  await pending;
  expect(mocks.revoke).toHaveBeenCalledOnce();
});
