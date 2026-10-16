/** Task summaries collect reading/delivery material; code stays in change views. */
const extensions = new Set(
  (
    "md markdown mdx txt rst adoc asciidoc org tex pdf doc docx odt rtf " +
    "html htm epub mobi ppt pptx odp key pages numbers xls xlsx ods csv tsv " +
    "png jpg jpeg gif webp svg avif heic bmp tiff tif " +
    "mp3 wav m4a aac flac ogg mp4 mov webm m4v"
  ).split(" "),
);
const documentNames = new Set([
  "readme",
  "license",
  "licence",
  "notice",
  "copying",
  "changelog",
  "changes",
  "authors",
  "contributing",
  "code_of_conduct",
  "security",
  "install",
  "todo",
]);
export function isTaskResourceFile(
  path: string,
  directory = false,
  attachmentName?: string,
): boolean {
  if (directory) return false;
  const name = path.split("/").at(-1)?.toLowerCase() ?? "";
  return (
    documentNames.has(name) ||
    extensions.has(name.includes(".") ? name.split(".").at(-1)! : "") ||
    (!name.includes(".") &&
      !!attachmentName &&
      isTaskResourceFile(attachmentName))
  );
}
