import { invoke } from "@tauri-apps/api/core";
import { native } from "./api";
import type { FileEntry, Request, Response } from "./protocol";
import { attachmentKind, type AttachmentKind } from "./attachmentKinds";
export {
  attachmentKind,
  attachmentLabels,
  type AttachmentKind,
} from "./attachmentKinds";

export interface ComposerReference extends FileEntry {
  attachmentKind?: AttachmentKind;
  mimeType?: string;
  size?: number;
}
export const MAX_PASTE_FILE = 64 * 1024 * 1024;
export function isLongPaste(text: string) {
  return text.length > 8000 || text.split("\n").length > 200;
}
export async function clipboardFilePaths(): Promise<string[]> {
  return native ? invoke("clipboard_file_paths") : [];
}
export async function importClipboardFile(
  hostId: string,
  projectId: string,
  path: string,
): Promise<ComposerReference> {
  const entry = await invoke<FileEntry>("import_clipboard_file", {
    hostId,
    projectId,
    path,
  });
  return { ...entry, attachmentKind: attachmentKind(entry.name) };
}
async function stage(request: Request): Promise<Response> {
  const response = await invoke<Response>("paste_stage", { request });
  if (response.kind === "error") throw new Error(response.message);
  return response;
}
export async function storePastedFile(
  file: File,
  hostId: string,
  valid: () => boolean,
  progress: (percent: number) => void,
  kind = attachmentKind(file.name, file.type),
): Promise<ComposerReference> {
  if (file.size > MAX_PASTE_FILE)
    throw new Error("单个粘贴附件不能超过 64 MiB");
  if (!native) throw new Error("粘贴附件需要在 Latte Work 桌面应用中使用");
  if (!valid()) throw new Error("粘贴已取消");
  const start = await stage({
    method: "begin_attachment",
    name: file.name,
    size: file.size,
  });
  if (start.kind !== "attachment_upload")
    throw new Error("Server 不支持粘贴附件，请更新后重试");
  try {
    for (let offset = 0; offset < file.size; offset += 65536) {
      if (!valid()) throw new Error("粘贴已取消");
      const data = Array.from(
        new Uint8Array(await file.slice(offset, offset + 65536).arrayBuffer()),
      );
      if (!valid()) throw new Error("粘贴已取消");
      await stage({ method: "attachment_chunk", id: start.id, offset, data });
      progress(
        Math.round(
          (Math.min(file.size, offset + data.length) / file.size) * 100,
        ),
      );
    }
    if (!valid()) throw new Error("粘贴已取消");
    progress(100);
    const entry = await invoke<FileEntry>("finish_paste_upload", {
      hostId,
      id: start.id,
    });
    return {
      ...entry,
      attachmentKind: kind,
      mimeType: file.type || undefined,
      size: file.size,
    };
  } catch (error) {
    await stage({ method: "abort_attachment", id: start.id }).catch(() => {});
    throw error;
  }
}
