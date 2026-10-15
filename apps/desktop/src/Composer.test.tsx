// @vitest-environment jsdom
import { useState } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Composer } from "./Composer";
import { composerTrigger } from "./composerActions";
import { referenceMessage } from "./referenceMessage";
const pasteMocks = vi.hoisted(() => ({
  paths: vi.fn(),
  importFile: vi.fn(),
  store: vi.fn(),
}));
vi.mock("./pasteAttachments", async (original) => ({
  ...(await original<typeof import("./pasteAttachments")>()),
  clipboardFilePaths: pasteMocks.paths,
  importClipboardFile: pasteMocks.importFile,
  storePastedFile: pasteMocks.store,
}));
const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  send: vi.fn(),
  choose: vi.fn(),
  openSubagents: vi.fn(),
  openModelSettings: vi.fn(),
}));
vi.mock("./api", () => ({
  request: mocks.request,
  chooseReferencePath: mocks.choose,
  message: (e: unknown) => String(e),
}));
function Harness({
  hostId = "remote",
  projectId = "p",
  agent = "claude",
}: {
  hostId?: string;
  projectId?: string;
  agent?: string;
}) {
  const [text, setText] = useState("");
  return (
    <Composer
      agent={agent}
      hostId={hostId}
      projectId={projectId}
      connected
      disabled={false}
      sending={false}
      hidden={false}
      value={text}
      onChange={setText}
      onSubmit={mocks.send}
      onOpenSubagents={mocks.openSubagents}
      onOpenModelSettings={mocks.openModelSettings}
      placeholder="任务"
      toolbar={(submit, content) => (
        <button disabled={!content} onClick={submit}>
          发送
        </button>
      )}
    />
  );
}
function type(value: string, cursor = value.length) {
  fireEvent.change(screen.getByRole("textbox", { name: "任务输入" }), {
    target: { value, selectionStart: cursor, selectionEnd: cursor },
  });
}
beforeEach(() => {
  vi.clearAllMocks();
  pasteMocks.paths.mockReset().mockResolvedValue([]);
  pasteMocks.store.mockReset();
  pasteMocks.importFile.mockReset();
  vi.stubGlobal("requestAnimationFrame", (fn: FrameRequestCallback) => {
    fn(0);
    return 0;
  });
  mocks.send.mockResolvedValue(false);
  mocks.openModelSettings.mockReset().mockReturnValue(null);
  mocks.choose.mockReset().mockResolvedValue(null);
  mocks.request.mockImplementation(async (_host, request) =>
    request.method === "agent_commands"
      ? {
          kind: "agent_commands",
          commands: [
            {
              name: "compact",
              description: "压缩上下文",
              argument_hint: "[说明]",
            },
            {
              name: "project:check",
              description: "项目检查",
              argument_hint: "<目标>",
            },
          ],
        }
      : {
          kind: "files",
          entries:
            request.path === "src"
              ? [
                  {
                    name: "中文 file.ts",
                    path: "src/中文 file.ts",
                    directory: false,
                  },
                ]
              : [
                  { name: "src", path: "src", directory: true },
                  { name: "README.md", path: "README.md", directory: false },
                ],
        },
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
it("detects only standalone mention and leading slash triggers", () => {
  expect(composerTrigger("mail@host", 9)).toBeNull();
  expect(composerTrigger("https://host/", 13)).toBeNull();
  expect(composerTrigger("/compact 参数", 11)).toBeNull();
  expect(composerTrigger("看看 @src/中文 file.ts 后文", 18)).toMatchObject({
    kind: "files",
    start: 3,
  });
});
it("loads native commands from the selected host and inserts without running; sends exact arguments", async () => {
  render(<Harness />);
  type("/comp");
  await screen.findByRole("option", { name: /compact/ });
  expect(mocks.request).toHaveBeenCalledWith("remote", {
    method: "agent_commands",
    project_id: "p",
    agent: "claude",
  });
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
    "/compact ",
  );
  expect(mocks.send).not.toHaveBeenCalled();
  type("/compact 保留结论");
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  await waitFor(() =>
    expect(mocks.send).toHaveBeenCalledWith("/compact 保留结论"),
  );
});
it("adds a file with spaces at the caret, preserves surrounding text and retries failed sends", async () => {
  render(<Harness />);
  type("检查 @src/中文 file.ts 后文", 18);
  fireEvent.click(await screen.findByRole("option", { name: /中文 file.ts/ }));
  expect(
    screen.getByRole("button", { name: "移除引用 src/中文 file.ts" }),
  ).toBeTruthy();
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
    "检查  后文",
  );
  fireEvent.click(screen.getByText("发送"));
  await waitFor(() => expect(mocks.send).toHaveBeenCalledOnce());
  expect(referenceMessage(mocks.send.mock.calls[0][0]).references).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ path: "src/中文 file.ts" }),
    ]),
  );
  expect(screen.getByRole("button", { name: /移除引用/ })).toBeTruthy();
  mocks.send.mockResolvedValue(true);
  fireEvent.click(screen.getByText("发送"));
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: /移除引用/ })).toBeNull(),
  );
});
it("supports plus folder browsing, folder references and removing chips", async () => {
  render(<Harness />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(await screen.findByRole("option", { name: /src/ }));
  fireEvent.click(await screen.findByRole("option", { name: /引用此文件夹/ }));
  expect(screen.getByRole("button", { name: "移除引用 src" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "移除引用 src" }));
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
});
it("does not submit on IME, empty results or menu selection and closes on Escape", async () => {
  render(<Harness />);
  type("/unknown");
  await screen.findByText(/没有匹配项/);
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect(mocks.send).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  type("中文输入");
  fireEvent.compositionStart(screen.getByRole("textbox"));
  fireEvent.keyDown(screen.getByRole("textbox"), {
    key: "Enter",
    keyCode: 229,
  });
  expect(mocks.send).not.toHaveBeenCalled();
});
it("discards late file responses and references when changing project or host", async () => {
  const view = render(<Harness />);
  type("@READ");
  fireEvent.click(await screen.findByRole("option", { name: /README/ }));
  expect(screen.getByLabelText("已添加项目引用")).toBeTruthy();
  let finish: (value: unknown) => void = () => {};
  mocks.request.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  type("@");
  await waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(2));
  view.rerender(<Harness hostId="another" projectId="different" />);
  await act(async () =>
    finish({
      kind: "files",
      entries: [{ name: "secret", path: "secret", directory: false }],
    }),
  );
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
  expect(screen.queryByRole("listbox")).toBeNull();
});
it("reports command discovery errors and retries against the agent", async () => {
  mocks.request.mockRejectedValueOnce(new Error("unsupported agent"));
  render(<Harness />);
  type("/");
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    expect.stringContaining("unsupported agent"),
  );
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  expect(await screen.findByRole("option", { name: /compact/ })).toBeTruthy();
});
it("keeps references through first-session creation while sending and clears on later navigation", async () => {
  const props = {
    agent: "claude",
    hostId: "local",
    projectId: "p",
    connected: true,
    disabled: false,
    sending: false,
    hidden: false,
    value: "",
    onChange: vi.fn(),
    onSubmit: mocks.send,
    placeholder: "任务",
    toolbar: () => null,
  };
  const view = render(<Composer {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(await screen.findByRole("option", { name: /README/ }));
  view.rerender(<Composer {...props} sending sessionId="new-session" />);
  expect(screen.getByLabelText("已添加项目引用")).toBeTruthy();
  view.rerender(<Composer {...props} sessionId="new-session" />);
  expect(screen.getByLabelText("已添加项目引用")).toBeTruthy();
  view.rerender(<Composer {...props} sessionId="another-session" />);
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
});
it("searches friendly titles but inserts the original agent command", async () => {
  render(<Harness />);
  type("/压缩");
  fireEvent.click(await screen.findByRole("option", { name: /压缩上下文/ }));
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
    "/compact ",
  );
  expect(mocks.send).not.toHaveBeenCalled();
});

it("references a local file or folder chosen outside the project without sending", async () => {
  mocks.choose
    .mockResolvedValueOnce("/outside/中文 file.txt")
    .mockResolvedValueOnce("/outside/docs");
  mocks.request.mockImplementation(async (_host, req) =>
    req.method === "resolve_reference"
      ? {
          kind: "file_reference",
          entry: {
            name: "chosen",
            path: req.path,
            directory: req.path.endsWith("docs"),
          },
        }
      : { kind: "files", entries: [] },
  );
  render(<Harness hostId="local" />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(screen.getByRole("button", { name: "工作区外引用…" }));
  fireEvent.click(screen.getByRole("button", { name: "选择文件…" }));
  await screen.findByRole("button", {
    name: "移除引用 /outside/中文 file.txt",
  });
  expect(mocks.choose).toHaveBeenCalledWith(false);
  expect(mocks.request).toHaveBeenCalledWith("local", {
    method: "resolve_reference",
    project_id: "p",
    path: "/outside/中文 file.txt",
  });
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(screen.getByRole("button", { name: "工作区外引用…" }));
  fireEvent.click(screen.getByRole("button", { name: "选择文件夹…" }));
  await screen.findByRole("button", { name: "移除引用 /outside/docs" });
  expect(mocks.choose).toHaveBeenCalledWith(true);
  expect(mocks.send).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("发送"));
  await waitFor(() => expect(mocks.send).toHaveBeenCalledOnce());
  expect(referenceMessage(mocks.send.mock.calls[0][0]).references).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ path: "/outside/中文 file.txt" }),
    ]),
  );
  expect(mocks.send.mock.calls[0][0]).toContain("绝对路径在当前主机");
});
it("resolves remote paths on the selected host and never opens a local chooser", async () => {
  mocks.request.mockResolvedValue({
    kind: "file_reference",
    entry: { name: "docs", path: "/home/user/docs", directory: true },
  });
  render(<Harness hostId="ssh:devbox" />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  expect(screen.queryByRole("button", { name: "选择文件…" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "引用远程路径…" }));
  const field = screen.getByRole("textbox", { name: "引用绝对路径" });
  fireEvent.change(field, { target: { value: "/home/user/docs" } });
  fireEvent.keyDown(field, { key: "Enter", isComposing: true });
  expect(mocks.request).not.toHaveBeenCalledWith(
    "ssh:devbox",
    expect.objectContaining({ method: "resolve_reference" }),
  );
  fireEvent.keyDown(field, { key: "Enter" });
  await screen.findByRole("button", { name: "移除引用 /home/user/docs" });
  expect(mocks.request).toHaveBeenCalledWith("ssh:devbox", {
    method: "resolve_reference",
    project_id: "p",
    path: "/home/user/docs",
  });
  expect(mocks.choose).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
});
it("keeps references unchanged on cancellation and rejects invalid or inaccessible paths", async () => {
  render(<Harness hostId="local" />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(screen.getByRole("button", { name: "工作区外引用…" }));
  fireEvent.click(screen.getByRole("button", { name: "选择文件…" }));
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "返回项目目录",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();

  const field = screen.getByRole("textbox", { name: "引用绝对路径" });
  fireEvent.change(field, { target: { value: "../secret" } });
  fireEvent.click(screen.getByRole("button", { name: "添加引用" }));
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    expect.stringContaining("绝对路径"),
  );
  mocks.request.mockRejectedValueOnce(new Error("路径不存在或无法访问"));
  fireEvent.change(field, { target: { value: "/missing" } });
  fireEvent.click(screen.getByRole("button", { name: "添加引用" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("路径不存在"),
  );
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
});
it("discards a late native selection after switching hosts", async () => {
  let finish: (value: string) => void = () => {};
  mocks.choose.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<Harness hostId="local" />);
  fireEvent.click(screen.getByRole("button", { name: "添加项目引用" }));
  fireEvent.click(screen.getByRole("button", { name: "工作区外引用…" }));
  fireEvent.click(screen.getByRole("button", { name: "选择文件…" }));
  view.rerender(<Harness hostId="remote" />);
  await act(async () => finish("/private/local-file"));
  expect(mocks.request).not.toHaveBeenCalledWith(
    expect.anything(),
    expect.objectContaining({ method: "resolve_reference" }),
  );
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
});

function pasteData(text = "", files: File[] = []) {
  fireEvent.paste(screen.getByRole("textbox", { name: "任务输入" }), {
    clipboardData: { files, getData: () => text },
  });
}
it("pastes short text at the selection and stages long text as a file without filling the input", async () => {
  render(<Harness hostId="local" />);
  type("prefix old suffix");
  const input = screen.getByRole("textbox", {
    name: "任务输入",
  }) as HTMLTextAreaElement;
  input.setSelectionRange(7, 10);
  pasteData("新文本");
  await waitFor(() => expect(input.value).toBe("prefix 新文本 suffix"));
  expect(pasteMocks.store).not.toHaveBeenCalled();
  pasteMocks.store.mockResolvedValue({
    name: "粘贴文本.txt",
    path: "/cache/text.txt",
    directory: false,
    attachmentKind: "text",
  });
  pasteData("x".repeat(8001));
  await screen.findByRole("button", { name: "移除引用 /cache/text.txt" });
  expect(input.value).toBe("prefix 新文本 suffix");
  expect(pasteMocks.store.mock.calls[0][0]).toBeInstanceOf(File);
  expect(pasteMocks.store.mock.calls[0][4]).toBe("text");
  expect(mocks.send).not.toHaveBeenCalled();
});
it("recognizes image/video/file pastes and keeps native file URLs ahead of clipboard previews", async () => {
  render(<Harness hostId="remote" />);
  pasteMocks.paths.mockResolvedValueOnce(["/local/movie.mov"]);
  pasteMocks.importFile.mockResolvedValue({
    name: "movie.mov",
    path: "/remote/cache/movie.mov",
    directory: false,
    attachmentKind: "video",
  });
  pasteData("movie.mov", [
    new File(["thumbnail"], "preview.png", { type: "image/png" }),
  ]);
  await screen.findByRole("button", {
    name: "移除引用 /remote/cache/movie.mov",
  });
  expect(pasteMocks.store).not.toHaveBeenCalled();
  expect(pasteMocks.importFile).toHaveBeenCalledWith(
    "remote",
    "p",
    "/local/movie.mov",
  );
  pasteMocks.store.mockImplementation(async (file: File) => ({
    name: file.name,
    path: `/remote/cache/${file.name}`,
    directory: false,
    attachmentKind: file.type.startsWith("image/") ? "image" : "file",
  }));
  pasteData("", [
    new File(["png"], "image.png", { type: "image/png" }),
    new File(["pdf"], "doc.pdf", { type: "application/pdf" }),
  ]);
  await screen.findByRole("button", {
    name: "移除引用 /remote/cache/image.png",
  });
  await screen.findByRole("button", { name: "移除引用 /remote/cache/doc.pdf" });
  expect(screen.getByText("图片 · image.png")).toBeTruthy();
  expect(screen.getByText("视频 · movie.mov")).toBeTruthy();
  expect(screen.getByText("文件 · doc.pdf")).toBeTruthy();
});
it("blocks send while pasting and discards failed batches and late results after host changes", async () => {
  const view = render(<Harness hostId="local" />);
  type("keep this draft");
  let finish: (value: unknown) => void = () => {};
  pasteMocks.store.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  pasteData("", [new File(["png"], "image.png")]);
  await waitFor(() => expect(pasteMocks.store).toHaveBeenCalledOnce());
  expect((screen.getByText("发送") as HTMLButtonElement).disabled).toBe(true);
  view.rerender(<Harness hostId="another" />);
  await act(async () =>
    finish({
      name: "image.png",
      path: "/old/cache/image.png",
      directory: false,
    }),
  );
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
  pasteMocks.store
    .mockResolvedValueOnce({
      name: "first.png",
      path: "/cache/first.png",
      directory: false,
    })
    .mockRejectedValueOnce(new Error("上传失败"));
  pasteData("", [new File(["1"], "first.png"), new File(["2"], "second.png")]);
  await screen.findByText(/上传失败/);
  expect(screen.queryByLabelText("已添加项目引用")).toBeNull();
  expect(
    (screen.getByRole("textbox", { name: "任务输入" }) as HTMLTextAreaElement)
      .value,
  ).toBe("keep this draft");
});

it("keeps pasted text authoritative when a clipboard manager restores older file URLs", async () => {
  pasteMocks.paths.mockResolvedValue(["/old/report.pdf"]);
  render(<Harness hostId="local" />);
  pasteData("ordinary text from this paste");
  await waitFor(() =>
    expect(
      (screen.getByRole("textbox", { name: "任务输入" }) as HTMLTextAreaElement)
        .value,
    ).toBe("ordinary text from this paste"),
  );
  expect(pasteMocks.importFile).not.toHaveBeenCalled();
});

it("shows one local subagent entry and hides native listing variants", async () => {
  mocks.request.mockResolvedValue({
    kind: "agent_commands",
    commands: [
      {
        name: "agents",
        display_name: "Manage native agents",
        description: "Native wizard",
        argument_hint: "",
        ui_action: "subagents",
      },
      {
        name: "list-agents",
        description: "Native list",
        argument_hint: "",
        ui_action: "subagents",
      },
      {
        name: "tasks",
        description: "Native tasks",
        argument_hint: "",
        ui_action: "subagents",
      },
      {
        name: "native-list",
        description: "Other adapter's native list",
        argument_hint: "",
        ui_action: "subagents",
      },
      { name: "compact", description: "压缩上下文", argument_hint: "" },
    ],
  });
  render(<Harness agent="another" />);
  type("/");
  await waitFor(() => expect(mocks.request).toHaveBeenCalled());
  await screen.findByRole("option", { name: /压缩上下文/ });
  expect(screen.getAllByRole("option")).toHaveLength(4);
  expect(
    screen.getByRole("option", { name: /子智能体/ }).textContent,
  ).toContain("/agents");
  expect(
    screen.queryByRole("option", {
      name: /list-agents|Native wizard|Native list|Native tasks/,
    }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("option", { name: /子智能体/ }));
  expect(mocks.openSubagents).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
  mocks.request.mockClear();
  fireEvent.click(screen.getByText("发送"));
  expect(mocks.openSubagents).toHaveBeenCalledOnce();
  expect(mocks.request).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
});

it("keeps local navigation available when native command discovery fails", async () => {
  mocks.request.mockRejectedValue(new Error("catalog unavailable"));
  render(<Harness agent="another" />);
  type("/");
  await screen.findByText(/catalog unavailable/);
  fireEvent.click(screen.getByRole("option", { name: /子智能体/ }));
  mocks.request.mockClear();
  fireEvent.click(screen.getByText("发送"));
  expect(mocks.openSubagents).toHaveBeenCalledOnce();
  expect(mocks.request).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
});

it("does not submit an adapter-specific navigation command discovered from pasted text", async () => {
  mocks.request.mockResolvedValue({
    kind: "agent_commands",
    commands: [
      {
        name: "native-list",
        description: "",
        argument_hint: "",
        ui_action: "subagents",
      },
    ],
  });
  render(<Harness agent="another" />);
  type("/native-list ");
  fireEvent.click(screen.getByText("发送"));
  await waitFor(() => expect(mocks.openSubagents).toHaveBeenCalledOnce());
  expect(mocks.send).not.toHaveBeenCalled();
});

it("fences a late discovery of adapter-specific navigation after changing projects", async () => {
  let finish!: (value: unknown) => void;
  mocks.request.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<Harness />);
  type("/native-list ");
  fireEvent.click(screen.getByText("发送"));
  await waitFor(() => expect(mocks.request).toHaveBeenCalled());
  view.rerender(<Harness projectId="other" />);
  await act(async () =>
    finish({
      kind: "agent_commands",
      commands: [
        {
          name: "native-list",
          description: "",
          argument_hint: "",
          ui_action: "subagents",
        },
      ],
    }),
  );
  expect(mocks.openSubagents).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
});

it.each([
  "/agents inspect",
  "/list-agents all",
  "/tasks 参数",
  "/agents\n请继续检查",
])(
  "never forwards navigation parameters in %s, and preserves the draft",
  async (command) => {
    render(<Harness agent="another" />);
    type(command);
    fireEvent.click(screen.getByText("发送"));
    expect(mocks.openSubagents).toHaveBeenCalledOnce();
    expect(mocks.send).not.toHaveBeenCalled();
    expect(mocks.request).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
      command,
    );
    expect(screen.getByText(/此命令不接受参数/)).toBeTruthy();
  },
);

it("prioritizes the exact native command over matches in other command descriptions", async () => {
  mocks.request.mockResolvedValue({
    kind: "agent_commands",
    commands: [
      { name: "batch", description: "Run many agents", argument_hint: "" },
      {
        name: "agents",
        description: "Manage agents",
        argument_hint: "",
        ui_action: "subagents",
      },
    ],
  });
  render(<Harness />);
  type("/agents");
  await screen.findAllByRole("option");
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
    "/agents ",
  );
  expect(mocks.send).not.toHaveBeenCalled();
});

it.each(["/list-agents", "/agents", "/tasks", "/subagents"])(
  "routes %s directly to the local panel without discovery or submission",
  async (command) => {
    render(<Harness />);
    type(` ${command}  `);
    fireEvent.click(screen.getByText("发送"));
    expect(mocks.openSubagents).toHaveBeenCalledOnce();
    expect(mocks.send).not.toHaveBeenCalled();
    expect(mocks.request).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("");
  },
);

it("maps a typed native alias to the single local menu entry", async () => {
  mocks.request.mockResolvedValue({
    kind: "agent_commands",
    commands: [
      {
        name: "list-agents",
        description: "List agents",
        argument_hint: "",
        ui_action: "subagents",
      },
    ],
  });
  render(<Harness />);
  type("/list-ag");
  await screen.findByRole("option", { name: /子智能体/ });
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect(mocks.openSubagents).not.toHaveBeenCalled();
  expect(mocks.send).not.toHaveBeenCalled();
  mocks.request.mockClear();
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect(mocks.openSubagents).toHaveBeenCalledOnce();
  expect(mocks.send).not.toHaveBeenCalled();
  expect(mocks.request).not.toHaveBeenCalled();
});

it("preserves attached references when a local panel command clears only its text", async () => {
  render(<Harness />);
  type("@README");
  fireEvent.click(await screen.findByRole("option", { name: /README.md/ }));
  type("/list-agents ");
  fireEvent.click(screen.getByText("发送"));
  expect(mocks.openSubagents).toHaveBeenCalledOnce();
  expect(mocks.send).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "移除引用 README.md" }),
  ).toBeTruthy();
});

it.each(["model", "effort"] as const)(
  "routes /%s to App settings without discovery or submission",
  (target) => {
    render(<Harness agent="another" />);
    type(` /${target}  `);
    fireEvent.click(screen.getByText("发送"));
    expect(mocks.openModelSettings).toHaveBeenCalledWith(target);
    expect(mocks.send).not.toHaveBeenCalled();
    expect(mocks.request).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("");
  },
);

it.each(["/model opus", "/effort high", "/model\n检查项目"])(
  "never dispatches App command parameters in %s",
  (command) => {
    render(<Harness />);
    type(command);
    fireEvent.click(screen.getByText("发送"));
    expect(mocks.send).not.toHaveBeenCalled();
    expect(mocks.request).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
      command,
    );
    expect(screen.getByText(/请在菜单中选择/)).toBeTruthy();
  },
);

it("preserves the draft when the App model settings are unavailable", () => {
  mocks.openModelSettings.mockReturnValue("当前 Agent 不支持调整思考强度。");
  render(<Harness />);
  type("/effort ");
  fireEvent.click(screen.getByText("发送"));
  expect(screen.getByText(/不支持调整思考强度/)).toBeTruthy();
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
    "/effort ",
  );
  expect(mocks.send).not.toHaveBeenCalled();
});

it("unifies native model and effort entries while retaining namespaced project commands", async () => {
  mocks.request.mockResolvedValue({
    kind: "agent_commands",
    commands: [
      { name: "model", description: "native model", argument_hint: "<id>" },
      {
        name: "effort",
        description: "native effort",
        argument_hint: "<level>",
      },
      {
        name: "project:model",
        description: "project model",
        argument_hint: "",
      },
    ],
  });
  render(<Harness />);
  type("/");
  await screen.findByRole("option", { name: /project model/ });
  expect(screen.getAllByRole("option")).toHaveLength(4);
  expect(
    screen.queryByRole("option", { name: /native model|native effort/ }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("option", { name: /选择模型/ }));
  expect(mocks.openModelSettings).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
  expect(mocks.openModelSettings).toHaveBeenCalledWith("model");
  expect(mocks.send).not.toHaveBeenCalled();
});

it("opens App settings when command discovery fails and preserves references", async () => {
  render(<Harness />);
  type("@README");
  fireEvent.click(await screen.findByRole("option", { name: /README.md/ }));
  mocks.request.mockRejectedValue(new Error("catalog unavailable"));
  type("/effort");
  await screen.findByText(/catalog unavailable/);
  fireEvent.click(screen.getByRole("option", { name: /思考强度/ }));
  mocks.request.mockClear();
  fireEvent.click(screen.getByText("发送"));
  expect(mocks.openModelSettings).toHaveBeenCalledWith("effort");
  expect(mocks.send).not.toHaveBeenCalled();
  expect(mocks.request).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "移除引用 README.md" }),
  ).toBeTruthy();
});

it("opens an attached image without sending or removing the draft and restores its trigger on close", async () => {
  URL.createObjectURL = vi.fn(() => "blob:composer");
  URL.revokeObjectURL = vi.fn();
  mocks.request.mockResolvedValue({
    kind: "source_preview",
    mime_type: "image/png",
    data: [137, 80, 78, 71],
    size: 4,
    next: 4,
    has_more: false,
  });
  render(
    <Composer
      agent="claude"
      hostId="remote"
      projectId="p"
      connected
      disabled={false}
      sending={false}
      hidden={false}
      value="这张图呢"
      onChange={vi.fn()}
      onSubmit={mocks.send}
      placeholder="任务"
      toolbar={() => null}
      referenceState={{
        scope: "remote:p:claude",
        entries: [
          {
            name: "image.png",
            path: "/remote/image.png",
            directory: false,
            attachmentKind: "image",
          },
        ],
      }}
      onReferencesChange={vi.fn()}
    />,
  );
  const trigger = screen.getByRole("button", { name: "预览图片 image.png" });
  fireEvent.click(trigger);
  await screen.findByRole("img", { name: "image.png" });
  expect(mocks.request).toHaveBeenCalledWith(
    "remote",
    expect.objectContaining({
      method: "preview_source",
      project_id: "p",
      path: "/remote/image.png",
    }),
  );
  expect(mocks.send).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(document.activeElement).toBe(trigger);
  expect(
    (screen.getByRole("textbox", { name: "任务输入" }) as HTMLTextAreaElement)
      .value,
  ).toBe("这张图呢");
  expect(
    screen.getByRole("button", { name: "移除引用 /remote/image.png" }),
  ).toBeTruthy();
});
