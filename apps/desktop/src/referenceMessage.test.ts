import { expect, it } from "vitest";
import type { ComposerReference } from "./pasteAttachments";
import {
  legacyReferenceMarker,
  referenceMarker,
  referencePrompt,
  referenceMessage,
} from "./referenceMessage";

const image = {
  name: "image.png",
  path: '/host/cache/uuid-image "新".png',
  directory: false,
  attachmentKind: "image" as const,
  mimeType: "image/png",
};
it("round-trips a compact manifest with host paths, directories, original upload names and escaped characters", () => {
  const references: ComposerReference[] = [
    image,
    { name: "src", path: "src", directory: true },
    { name: "code.ts", path: "src/code.ts", directory: false },
  ];
  const prompt = referencePrompt("  看看这些文件  ", references);
  expect(prompt.split("\n")).toHaveLength(4);
  expect(prompt).toContain("按现有权限读取");
  expect(prompt).not.toContain("mime_type");
  expect(prompt).not.toContain('"name":"code.ts"');
  const content = referenceMessage(prompt);
  expect(content.text).toBe("看看这些文件");
  expect(content.references).toMatchObject(
    references.map(({ mimeType, ...value }) => value),
  );
  expect(referencePrompt(" x ", [])).toBe("x");
});

it("recognizes legacy image messages and retains their path and MIME without exposing the metadata as prose", () => {
  const prompt = `之前的问题${legacyReferenceMarker}${JSON.stringify([{ name: "image.png", path: "/remote/image.png", type: "file", kind: "image", mime_type: "image/png" }], null, 2)}`;
  expect(referenceMessage(prompt)).toEqual({
    text: "之前的问题",
    references: [
      {
        name: "image.png",
        path: "/remote/image.png",
        directory: false,
        attachmentKind: "image",
        mimeType: "image/png",
      },
    ],
  });
  const compact = referencePrompt("之前的问题", [
    { ...image, path: "/remote/image.png" },
  ]);
  expect(compact.length).toBeLessThan(prompt.length * 0.6);
});

it.each([
  "not JSON",
  "[]",
  '[{"path":"/x","type":"other"}]',
  '[{"path":"","type":"file"}]',
  '[{"path":"/x","type":"file","extra":"important prose"}]',
  JSON.stringify(Array(21).fill({ path: "/x", type: "file" })),
  '[{"path":"/x","type":"file"}]\n后面的真实内容',
])("keeps invalid, oversized or incomplete metadata visible: %s", (raw) => {
  const text = `这是一段普通文本${referenceMarker}${raw}`;
  expect(referenceMessage(text)).toEqual({ text, references: [] });
});

it("does not turn arbitrary prose or file URLs into attachments", () => {
  const text =
    '图片 /private/image.png\n[{"path":"/private/image.png","type":"file"}]';
  expect(referenceMessage(text)).toEqual({ text, references: [] });
});
