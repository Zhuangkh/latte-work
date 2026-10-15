import { expect, it } from "vitest";
import { projectFolderName } from "./projectFolderName";

it.each([
  ["C:\\Users\\alice\\项目\\", "项目"],
  ["C:/Users/alice/project/", "project"],
  ["\\\\?\\C:\\long path\\项目", "项目"],
  ["C:\\", "C:\\"],
  ["\\\\?\\C:\\", "C:\\"],
  ["\\\\server\\share\\", "share"],
  ["\\\\?\\UNC\\server\\share\\folder", "folder"],
  ["C:\\parent\\" + "long parent ".repeat(20) + "\\demo", "demo"],
])("names a manually entered Windows path %s", (path, expected) => {
  expect(projectFolderName(path, "windows")).toBe(expected);
});

it.each([
  ["/home/alice/project/", "project"],
  ["/tmp/foo\\bar", "foo\\bar"],
  ["/tmp/C:\\project", "C:\\project"],
  ["/", "/"],
  ["/tmp/project/.", "project"],
])("preserves POSIX host semantics for %s", (path, expected) => {
  expect(projectFolderName(path, "posix")).toBe(expected);
});

it("does not infer host syntax or invent names for incomplete paths", () => {
  expect(projectFolderName("C:\\project", undefined)).toBeUndefined();
  expect(projectFolderName("/tmp/foo\\bar", undefined)).toBeUndefined();
  expect(projectFolderName("C:project", "windows")).toBeUndefined();
  expect(projectFolderName("C:\\project", "posix")).toBeUndefined();
  expect(projectFolderName("/tmp/project/..", "posix")).toBeUndefined();
  expect(projectFolderName("", "windows")).toBeUndefined();
});
