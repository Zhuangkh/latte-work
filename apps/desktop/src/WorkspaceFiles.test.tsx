// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { renderWithStatus as render } from "./test/renderWithStatus";
import { afterEach, expect, it, vi } from "vitest";
import { useState } from "react";
import { WorkspaceFiles } from "./WorkspaceFiles";
const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("./api", () => ({ request: mocks.request, message: String }));
HTMLElement.prototype.scrollIntoView = vi.fn();
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});
function Harness({ tab, file = "" }: { tab: "files" | "diff"; file?: string }) {
  const [location, setLocation] = useState({ path: "", file });
  return (
    <WorkspaceFiles
      hostId="local"
      project={{ id: "p", name: "Project", path: "/project" }}
      active
      tab={tab}
      {...location}
      navigate={(next) => setLocation({ ...location, ...next })}
    />
  );
}
it("keeps the same right tree before opening a file and after a preview fails", async () => {
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file") throw new Error("仅预览文本文件");
    return {
      kind: "files",
      entries: [{ name: "binary", path: "binary", directory: false }],
    };
  });
  const view = render(<Harness tab="files" />);
  const tree = screen.getByRole("navigation", { name: "项目文件树" });
  const separator = screen.getByRole("separator", { name: "调整文件目录宽度" });
  expect(tree.previousElementSibling).toBe(separator);
  expect(separator.previousElementSibling?.className).toBe(
    "workspace-file-content",
  );
  expect(screen.getByText("从右侧目录选择文件")).toBeTruthy();
  expect(view.container.querySelector(".file-list")).toBeNull();
  fireEvent.click(await screen.findByRole("button", { name: "binary" }));
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(screen.getByRole("dialog", { name: "状态提示" })).toHaveProperty(
    "textContent",
    expect.stringContaining("仅预览文本"),
  );
  fireEvent.click(screen.getByRole("button", { name: "返回项目目录" }));
  expect(await screen.findByRole("button", { name: "binary" })).toBeTruthy();
  expect(screen.getByRole("navigation", { name: "项目文件树" })).toBe(tree);
  expect(screen.getByText("从右侧目录选择文件")).toBeTruthy();
  expect(
    mocks.request.mock.calls.filter(([, r]) => r.method === "files"),
  ).toHaveLength(1);
});
it("switches files from the tree and keeps the preview when the tree is hidden", async () => {
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file")
      return { kind: "content", text: `Content of ${r.path}`, truncated: true };
    return {
      kind: "files",
      entries: [
        { name: "README.md", path: "README.md", directory: false },
        { name: "notes.txt", path: "notes.txt", directory: false },
      ],
    };
  });
  render(<Harness tab="files" file="README.md" />);
  expect(screen.queryByText("项目文件")).toBeNull();
  expect(screen.queryByRole("button", { name: "返回目录" })).toBeNull();
  const header = screen
    .getByRole("button", { name: "隐藏文件树" })
    .closest<HTMLDivElement>(".workspace-file-header")!;
  expect(within(header).getByText("README.md")).toBeTruthy();
  expect(within(header).getByRole("button", { name: "刷新" })).toBeTruthy();
  const tree = screen.getByRole("navigation", { name: "项目文件树" });
  const selected = await within(tree).findByRole("button", {
    name: "README.md",
  });
  expect(selected.getAttribute("aria-current")).toBe("page");
  expect(screen.queryByText(/只读预览/)).toBeNull();
  expect(await screen.findByText("内容已截断")).toBeTruthy();
  expect(within(header).getByText("内容已截断")).toBeTruthy();
  fireEvent.click(within(tree).getByRole("button", { name: "notes.txt" }));
  expect(await screen.findByText("Content of notes.txt")).toBeTruthy();
  expect(
    within(tree)
      .getByRole("button", { name: "notes.txt" })
      .getAttribute("aria-current"),
  ).toBe("page");
  fireEvent.click(screen.getByRole("button", { name: "隐藏文件树" }));
  expect(screen.queryByRole("navigation", { name: "项目文件树" })).toBeNull();
  expect(screen.getByText("Content of notes.txt")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "显示文件树" }));
  expect(
    await within(tree).findByRole("button", { name: "notes.txt" }),
  ).toBeTruthy();
});
it("wraps files by default without changing their text or rereading on toggle", async () => {
  const content = `\t  ${"long_identifier_".repeat(30)}\n  中文预览保留缩进  `;
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file")
      return {
        kind: "content",
        text: r.path === "code.txt" ? content : "Next file",
        truncated: false,
      };
    return {
      kind: "files",
      entries: [
        { name: "code.txt", path: "code.txt", directory: false },
        { name: "next.txt", path: "next.txt", directory: false },
      ],
    };
  });
  render(<Harness tab="files" file="code.txt" />);
  const preview = await screen.findByRole("region", { name: "文件内容" });
  const toggle = screen.getByRole("button", { name: "自动换行" });
  expect(toggle.getAttribute("aria-pressed")).toBe("true");
  expect(preview.classList.contains("wrap")).toBe(true);
  expect(
    Array.from(preview.querySelectorAll("code"), (line) => line.textContent),
  ).toEqual(content.split("\n"));
  expect(
    Array.from(
      preview.querySelectorAll(".line-number"),
      (line) => line.textContent,
    ),
  ).toEqual(["1", "2"]);
  const calls = mocks.request.mock.calls.length;
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  expect(preview.classList.contains("wrap")).toBe(false);
  expect(mocks.request).toHaveBeenCalledTimes(calls);
  fireEvent.click(await screen.findByRole("button", { name: "next.txt" }));
  expect(await screen.findByText("Next file")).toBeTruthy();
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
});
it("restores the file wrapping preference when the file page reopens", async () => {
  mocks.request.mockResolvedValue({ kind: "files", entries: [] });
  const view = render(<Harness tab="files" />);
  fireEvent.click(screen.getByRole("button", { name: "自动换行" }));
  view.unmount();
  render(<Harness tab="files" />);
  const toggle = screen.getByRole("button", { name: "自动换行" });
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-pressed")).toBe("true");
});
it("expands the selected file's ancestors and reads other folders only when opened", async () => {
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file")
      return {
        kind: "content",
        text: `Content of ${r.path}`,
        truncated: false,
      };
    return {
      kind: "files",
      entries:
        r.path === ""
          ? [
              { name: "src", path: "src", directory: true },
              { name: "docs", path: "docs", directory: true },
            ]
          : [
              {
                name: "index.ts",
                path: `${r.path}/index.ts`,
                directory: false,
              },
            ],
    };
  });
  render(<Harness tab="files" file="src/index.ts" />);
  const tree = screen.getByRole("navigation", { name: "项目文件树" });
  expect(
    (
      await within(tree).findByRole("button", { name: "index.ts" })
    ).getAttribute("aria-current"),
  ).toBe("page");
  expect(mocks.request).not.toHaveBeenCalledWith("local", {
    method: "files",
    project_id: "p",
    path: "docs",
  });
  fireEvent.click(within(tree).getByRole("button", { name: "docs" }));
  await waitFor(() =>
    expect(
      within(tree).getAllByRole("button", { name: "index.ts" }),
    ).toHaveLength(2),
  );
  fireEvent.click(screen.getByRole("button", { name: "隐藏文件树" }));
  fireEvent.click(screen.getByRole("button", { name: "显示文件树" }));
  await waitFor(() =>
    expect(within(tree).queryAllByRole("status")).toHaveLength(0),
  );
  expect(
    within(tree)
      .getByRole("button", { name: "docs" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  fireEvent.click(within(tree).getAllByRole("button", { name: "index.ts" })[1]);
  expect(await screen.findByText("Content of docs/index.ts")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "返回项目目录" }));
  expect(screen.getByRole("navigation", { name: "项目文件树" })).toBe(tree);
  expect(
    within(tree).getAllByRole("button", { name: "index.ts" }),
  ).toHaveLength(2);
  expect(
    within(tree)
      .getByRole("button", { name: "docs" })
      .getAttribute("aria-expanded"),
  ).toBe("true");
  expect(screen.getByText("从右侧目录选择文件")).toBeTruthy();
  expect(mocks.request).toHaveBeenLastCalledWith("local", {
    method: "read_file",
    project_id: "p",
    path: "docs/index.ts",
  });
});
it("retries a failed tree independently of the open file and refreshes both", async () => {
  let fail = true;
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file")
      return { kind: "content", text: "Preview remains", truncated: false };
    if (fail) throw new Error("目录不可访问");
    return {
      kind: "files",
      entries: [{ name: "README.md", path: "README.md", directory: false }],
    };
  });
  render(<Harness tab="files" file="README.md" />);
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("目录不可访问");
  expect(await screen.findByText("Preview remains")).toBeTruthy();
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  await within(screen.getByRole("navigation")).findByRole("button", {
    name: "README.md",
  });
  mocks.request.mockClear();
  fireEvent.click(screen.getByRole("button", { name: "刷新" }));
  await waitFor(() =>
    expect(mocks.request).toHaveBeenCalledWith("local", {
      method: "read_file",
      project_id: "p",
      path: "README.md",
    }),
  );
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "files",
    project_id: "p",
    path: "",
  });
});
it("keeps the Git file list available after a failed diff preview", async () => {
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "git_info")
      return {
        kind: "git_info",
        info: {
          branch: "main",
          head: "a".repeat(40),
          default_base: "HEAD",
          refs: [],
          truncated: false,
        },
      };
    if (r.method === "git_review_diff") throw new Error("文件已移走");
    return {
      kind: "git_review",
      review: {
        base: "a".repeat(40),
        head: "a".repeat(40),
        added: 2,
        removed: 1,
        truncated: false,
        entries: [
          {
            path: "file.txt",
            previous_path: null,
            status: "M",
            added: 1,
            removed: 1,
            binary: false,
            untracked: false,
          },
          {
            path: "new.txt",
            previous_path: null,
            status: "A",
            added: 1,
            removed: 0,
            binary: false,
            untracked: false,
          },
        ],
      },
    };
  });
  render(<Harness tab="diff" />);
  await within(
    screen.getByRole("complementary", { name: "变更文件" }),
  ).findByRole("button", { name: /file.txt/ });
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("文件已移走");
  expect(mocks.request).toHaveBeenCalledWith(
    "local",
    expect.objectContaining({
      method: "git_review_diff",
      project_id: "p",
      path: "file.txt",
      scope: "branch",
    }),
  );
  fireEvent.click(
    within(screen.getByRole("complementary", { name: "变更文件" })).getByRole(
      "button",
      { name: /new.txt/ },
    ),
  );
  expect(
    within(screen.getByRole("complementary", { name: "变更文件" })).getByRole(
      "button",
      { name: /file.txt/ },
    ),
  ).toBeTruthy();
  await waitFor(() =>
    expect(mocks.request).toHaveBeenLastCalledWith(
      "local",
      expect.objectContaining({ path: "new.txt" }),
    ),
  );
});
it("withdraws a failed file preview after switching to the changes tab", async () => {
  mocks.request.mockImplementation(async (_host, req) => {
    if (req.method === "read_file") throw new Error("Preview failed");
    if (req.method === "files") return { kind: "files", entries: [] };
    if (req.method === "git_info")
      return {
        kind: "git_info",
        info: {
          branch: "main",
          head: "a".repeat(40),
          default_base: "HEAD",
          refs: [],
          truncated: false,
        },
      };
    return {
      kind: "git_review",
      review: {
        base: "a".repeat(40),
        head: "a".repeat(40),
        added: 0,
        removed: 0,
        truncated: false,
        entries: [],
      },
    };
  });
  const view = render(<Harness tab="files" file="README.md" />);
  fireEvent.click(await screen.findByRole("button", { name: "状态提示（1）" }));
  expect(screen.getByText("Error: Preview failed")).toBeTruthy();
  view.rerender(<Harness tab="diff" />);
  await screen.findByText("此范围没有变更");
  expect(screen.queryByText("Error: Preview failed")).toBeNull();
  expect(screen.getByRole("button", { name: "状态提示" })).toBeTruthy();
});
it("does not report a clean worktree when the server cannot enumerate changes", async () => {
  mocks.request.mockRejectedValue(new Error("不支持 changes"));
  render(<Harness tab="diff" />);
  fireEvent.click(await screen.findByRole("button", { name: /^状态提示（/ }));
  expect(
    screen.getByRole("dialog", { name: "状态提示" }).textContent,
  ).toContain("不支持");
  expect(screen.queryByText(/工作区干净/)).toBeNull();
});

it("opens a Skill's SKILL.md directly in the shared file browser and navigates its real tree", async () => {
  const root = "/host/skills/review";
  mocks.request.mockImplementation(async (_host, r) => {
    expect(r.method).toBe("preview_source");
    if (r.path === root)
      return {
        kind: "source_directory",
        entries: [
          { path: `${root}/SKILL.md`, name: "SKILL.md", directory: false },
          { path: `${root}/notes.txt`, name: "notes.txt", directory: false },
        ],
        truncated: false,
      };
    const data = Array.from(
      new TextEncoder().encode(
        r.path.endsWith("SKILL.md") ? "Skill description" : "Supporting notes",
      ),
    );
    return {
      kind: "source_preview",
      data,
      next: data.length,
      size: data.length,
      has_more: false,
      mime_type: "text/plain",
    };
  });
  function SkillBrowser() {
    const [file, setFile] = useState(`${root}/SKILL.md`);
    return (
      <WorkspaceFiles
        hostId="ssh"
        project={{ id: "p", name: "Project", path: "/project" }}
        tab="files"
        active
        reference={{ path: root, name: "review", directory: true }}
        sessionId="s"
        path={root}
        file={file}
        navigate={(value) => setFile(value.file ?? "")}
      />
    );
  }
  render(<SkillBrowser />);
  expect(await screen.findByText("Skill description")).toBeTruthy();
  const tree = screen.getByRole("navigation", { name: "项目文件树" });
  expect(
    within(tree)
      .getByRole("button", { name: "SKILL.md" })
      .getAttribute("aria-current"),
  ).toBe("page");
  fireEvent.click(within(tree).getByRole("button", { name: "notes.txt" }));
  expect(await screen.findByText("Supporting notes")).toBeTruthy();
  expect(screen.getByRole("button", { name: "自动换行" })).toBeTruthy();
  expect(mocks.request).toHaveBeenCalledWith(
    "ssh",
    expect.objectContaining({ session_id: "s", path: `${root}/SKILL.md` }),
  );
});
it("reads an explicitly selected external file without browsing its parent directory", async () => {
  const path = "/private/tmp/report.md";
  const data = Array.from(new TextEncoder().encode("Report body"));
  mocks.request.mockResolvedValue({
    kind: "source_preview",
    data,
    next: data.length,
    size: data.length,
    has_more: false,
    mime_type: "text/plain",
  });
  render(
    <WorkspaceFiles
      hostId="ssh"
      project={{ id: "p", name: "P", path: "/project" }}
      tab="files"
      active
      path="/private/tmp"
      file={path}
      reference={{ path, name: "report.md", directory: false }}
      sessionId="s"
      navigate={vi.fn()}
    />,
  );
  expect(await screen.findByText("Report body")).toBeTruthy();
  expect(mocks.request).toHaveBeenCalledExactlyOnceWith("ssh", {
    method: "preview_source",
    project_id: "p",
    session_id: "s",
    path,
    offset: 0,
  });
});

it("releases a pending project read when switching to an external reference", async () => {
  mocks.request.mockImplementation(async (_host, r) => {
    if (r.method === "read_file") return new Promise(() => {});
    if (r.method === "files") return { kind: "files", entries: [] };
    return {
      kind: "source_preview",
      data: [79, 75],
      size: 2,
      next: 2,
      has_more: false,
      mime_type: "text/plain",
    };
  });
  const base = {
    hostId: "ssh",
    project: { id: "p", name: "P", path: "/project" },
    tab: "files" as const,
    active: true,
    path: "",
    navigate: vi.fn(),
  };
  const view = render(<WorkspaceFiles {...base} file="pending.md" />);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "刷新" }).hasAttribute("disabled"),
    ).toBe(true),
  );
  view.rerender(
    <WorkspaceFiles
      {...base}
      file="/tmp/report.md"
      reference={{
        name: "report.md",
        path: "/tmp/report.md",
        directory: false,
      }}
    />,
  );
  expect(await screen.findByText("OK")).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "刷新" }).hasAttribute("disabled"),
  ).toBe(false);
  expect(screen.queryByRole("button", { name: "返回所选目录" })).toBeNull();
});
