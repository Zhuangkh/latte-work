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
import { ProjectDialog } from "./ProjectDialog";
import type { Host, ProjectFolder } from "./api";
import type { PathSyntax } from "./protocol";
import type { Workbench } from "./useWorkbench";

const mocks = vi.hoisted(() => ({
  pick: vi.fn(),
  connect: vi.fn(),
  request: vi.fn(),
  submit: vi.fn(),
}));
vi.mock("./api", () => ({
  chooseProjectFolder: mocks.pick,
  connect: mocks.connect,
  request: mocks.request,
  message: String,
}));
const local: Host = { id: "local", name: "本机", ssh: null, server_path: null };
const remote: Host = {
  id: "remote",
  name: "Remote",
  ssh: "remote",
  server_path: null,
};

function Harness({
  host = local,
  syntax,
  initialName = "",
  initialPath = "",
}: {
  host?: Host;
  syntax?: PathSyntax;
  initialName?: string;
  initialPath?: string;
}) {
  const [projectPath, setProjectPath] = useState(initialPath);
  const [projectName, setProjectName] = useState(initialName);
  const [error, setError] = useState("");
  return (
    <ProjectDialog
      state={
        {
          hosts: [local, remote],
          projectHostId: host.id,
          setProjectHostId: vi.fn(),
          projectPath,
          setProjectPath,
          projectName,
          setProjectName,
          hostPathSyntax: () => syntax,
          busy: false,
          error,
          setError,
          setModal: vi.fn(),
          addProject: (event: React.FormEvent) => {
            event.preventDefault();
            mocks.submit({
              hostId: host.id,
              path: projectPath,
              name: projectName,
            });
          },
        } as unknown as Workbench
      }
    />
  );
}
const title = (container: HTMLElement) =>
  container.querySelector(".chosen-folder strong")?.textContent;
afterEach(cleanup);
beforeEach(() => {
  vi.resetAllMocks();
  mocks.connect.mockResolvedValue({ kind: "hello" });
});

it("uses the native basename for a long Windows selection and submits the original path", async () => {
  const folder = {
    path: `C:\\${"parent ".repeat(25)}\\中文 project`,
    name: "中文 project",
  };
  mocks.pick.mockResolvedValue(folder);
  const { container } = render(<Harness syntax="windows" />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() =>
    expect((screen.getByLabelText("项目名称") as HTMLInputElement).value).toBe(
      folder.name,
    ),
  );
  expect(title(container)).toBe(folder.name);
  fireEvent.click(screen.getByRole("button", { name: "创建项目" }));
  expect(mocks.submit).toHaveBeenCalledWith({ hostId: "local", ...folder });
});

it("preserves a custom name when a folder is selected and when manual input changes", async () => {
  mocks.pick.mockResolvedValue({ path: "C:\\project", name: "project" });
  const { container } = render(
    <Harness syntax="windows" initialName="自定义" />,
  );
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() => expect(title(container)).toBe("project"));
  fireEvent.change(screen.getByLabelText("项目绝对路径"), {
    target: { value: "D:\\其他\\" },
  });
  expect(title(container)).toBe("其他");
  expect((screen.getByLabelText("项目名称") as HTMLInputElement).value).toBe(
    "自定义",
  );
  expect(mocks.request).not.toHaveBeenCalled();
});

it("discards a local picker result that arrives after switching hosts", async () => {
  let finish!: (folder: ProjectFolder) => void;
  mocks.pick.mockImplementation(
    () =>
      new Promise<ProjectFolder>((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<Harness syntax="windows" />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  view.rerender(<Harness host={remote} syntax="posix" />);
  await act(async () => finish({ path: "C:\\local", name: "local" }));
  expect((screen.getByLabelText("项目名称") as HTMLInputElement).value).toBe(
    "",
  );
  expect(
    (screen.getByLabelText("项目绝对路径") as HTMLInputElement).value,
  ).toBe("");
});

it("uses the server's current-directory name even without a syntax declaration", async () => {
  mocks.request.mockResolvedValue({
    kind: "directories",
    path: "/tmp/foo\\bar",
    name: "foo\\bar",
    parent: "/tmp",
    entries: [],
    truncated: false,
  });
  const { container } = render(<Harness host={remote} />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "选择此文件夹",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "选择此文件夹" }));
  expect(title(container)).toBe("foo\\bar");
  expect((screen.getByLabelText("项目名称") as HTMLInputElement).value).toBe(
    "foo\\bar",
  );
  expect(mocks.request).toHaveBeenCalledWith("remote", {
    method: "browse_directories",
    path: null,
  });
});

it("uses an entry's host-provided name when browsing an older server", async () => {
  mocks.request
    .mockResolvedValueOnce({
      kind: "directories",
      path: "/tmp",
      parent: "/",
      entries: [{ path: "/tmp/foo\\bar", name: "foo\\bar", directory: true }],
      truncated: false,
    })
    .mockResolvedValueOnce({
      kind: "directories",
      path: "/tmp/foo\\bar",
      parent: "/tmp",
      entries: [],
      truncated: false,
    });
  const { container } = render(<Harness host={remote} />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  fireEvent.click(await screen.findByRole("button", { name: "foo\\bar" }));
  await waitFor(() =>
    expect(
      (screen.getByLabelText("远程目录路径") as HTMLInputElement).value,
    ).toBe("/tmp/foo\\bar"),
  );
  fireEvent.click(screen.getByRole("button", { name: "选择此文件夹" }));
  expect(title(container)).toBe("foo\\bar");
});

it("parses manual titles with the target host's syntax and forgets a selected name after editing", async () => {
  mocks.pick.mockResolvedValue({ path: "C:\\picked", name: "native name" });
  const view = render(<Harness syntax="windows" />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() => expect(title(view.container)).toBe("native name"));
  fireEvent.change(screen.getByLabelText("项目绝对路径"), {
    target: { value: "C:\\manual\\" },
  });
  expect(title(view.container)).toBe("manual");
  view.rerender(<Harness host={remote} syntax="posix" />);
  fireEvent.change(screen.getByLabelText("项目绝对路径"), {
    target: { value: "/tmp/foo\\bar/" },
  });
  expect(title(view.container)).toBe("foo\\bar");
  view.rerender(<Harness host={remote} />);
  expect(title(view.container)).toBe("项目文件夹");
  expect(mocks.connect).not.toHaveBeenCalled();
});

it("retains a newly browsed host's declaration for subsequent manual input", async () => {
  mocks.connect.mockResolvedValue({ kind: "hello", path_syntax: "posix" });
  mocks.request.mockResolvedValue({
    kind: "directories",
    path: "/tmp",
    name: "tmp",
    parent: "/",
    entries: [],
    truncated: false,
  });
  const { container } = render(<Harness host={remote} />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "选择此文件夹",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "选择此文件夹" }));
  fireEvent.change(screen.getByLabelText("项目绝对路径"), {
    target: { value: "/tmp/foo\\bar" },
  });
  expect(title(container)).toBe("foo\\bar");
});

it("keeps an unnamed legacy current directory selectable without guessing its name", async () => {
  mocks.request.mockResolvedValue({
    kind: "directories",
    path: "/",
    parent: null,
    entries: [],
    truncated: false,
  });
  const { container } = render(<Harness host={remote} />);
  fireEvent.click(screen.getByRole("button", { name: "添加" }));
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "选择此文件夹",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "选择此文件夹" }));
  expect(title(container)).toBe("项目文件夹");
  expect((screen.getByLabelText("项目名称") as HTMLInputElement).value).toBe(
    "",
  );
});
