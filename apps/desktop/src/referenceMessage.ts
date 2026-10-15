import type { ComposerReference } from "./pasteAttachments";
import { attachmentKind } from "./attachmentKinds";

export const referenceMarker =
  "\n\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n";
export const legacyReferenceMarker =
  "\n\n文件引用（相对路径基于当前项目根目录；绝对路径位于当前执行主机。仅引用路径，按当前权限读取）：\n";

/** The same bounded manifest is sent to the Agent and persisted with the request. */
export function referencePrompt(text: string, references: ComposerReference[]) {
  if (!references.length) return text.trim();
  return `${text.trim() || "请查看所附文件。"}${referenceMarker}${JSON.stringify(
    references.map((entry) => ({
      path: entry.path,
      type: entry.directory ? "directory" : "file",
      ...(entry.name !== entry.path.split("/").at(-1)
        ? { name: entry.name }
        : {}),
      ...(!entry.directory && referenceKind(entry) !== "file"
        ? { kind: referenceKind(entry) }
        : {}),
    })),
  )}`;
}

export function referenceKind(entry: ComposerReference) {
  return entry.attachmentKind ?? attachmentKind(entry.name, entry.mimeType);
}

/** Only hide a complete App manifest; malformed or arbitrary prose stays intact. */
export function referenceMessage(text: string): {
  text: string;
  references: ComposerReference[];
} {
  const marker = [referenceMarker, legacyReferenceMarker]
    .map((value) => ({ value, index: text.lastIndexOf(value) }))
    .sort((a, b) => b.index - a.index)[0];
  const original = { text, references: [] };
  if (marker.index < 0) return original;
  const raw = text.slice(marker.index + marker.value.length);
  if (raw.length > 128 * 1024) return original;
  try {
    const entries: unknown = JSON.parse(raw);
    if (!Array.isArray(entries) || !entries.length || entries.length > 20)
      return original;
    const references: ComposerReference[] = [];
    for (const entry of entries) {
      if (
        !entry ||
        typeof entry !== "object" ||
        Array.isArray(entry) ||
        Object.keys(entry).some(
          (key) => !["path", "type", "name", "kind", "mime_type"].includes(key),
        ) ||
        typeof entry.path !== "string" ||
        !entry.path ||
        entry.path.length > 4096 ||
        entry.path.includes("\0") ||
        !["file", "directory"].includes(entry.type) ||
        (entry.kind !== undefined &&
          !["image", "video", "file", "text"].includes(entry.kind)) ||
        (entry.name !== undefined &&
          (typeof entry.name !== "string" ||
            !entry.name ||
            entry.name.length > 256)) ||
        (entry.mime_type !== undefined &&
          (typeof entry.mime_type !== "string" || entry.mime_type.length > 128))
      )
        return original;
      references.push({
        path: entry.path,
        name: entry.name ?? entry.path.split("/").at(-1) ?? entry.path,
        directory: entry.type === "directory",
        attachmentKind: entry.kind,
        mimeType: entry.mime_type,
      });
    }
    return { text: text.slice(0, marker.index), references };
  } catch {
    return original;
  }
}
