export type AttachmentKind = "image" | "video" | "file" | "text";
export function attachmentKind(name: string, mime = ""): AttachmentKind {
  if (
    mime.startsWith("image/") ||
    /\.(png|jpe?g|gif|webp|bmp|tiff?|heic|avif|svg)$/i.test(name)
  )
    return "image";
  if (
    mime.startsWith("video/") ||
    /\.(mp4|mov|webm|mkv|avi|m4v|mpeg|mpg)$/i.test(name)
  )
    return "video";
  return "file";
}
export const attachmentLabels: Record<AttachmentKind, string> = {
  image: "图片",
  video: "视频",
  file: "文件",
  text: "粘贴文本",
};
