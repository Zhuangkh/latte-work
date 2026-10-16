// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SSHSettings } from "./SSHSettings";
import type { Host } from "./api";
import type { Workbench } from "./useWorkbench";

vi.mock("./MessageContent", () => ({ CopyButton: () => null }));
afterEach(cleanup);

function setup(
  hosts: Host[] = [],
  saveSshHost = vi.fn().mockResolvedValue(undefined),
) {
  const state = { hosts, hostErrors: {}, saveSshHost } as unknown as Workbench;
  render(<SSHSettings state={state} onBusy={vi.fn()} />);
  return saveSshHost;
}

describe("SSH remote platform", () => {
  it("saves Windows platform and a literal executable path, then disables controls while connecting", async () => {
    let complete!: () => void;
    const save = setup(
      [],
      vi.fn().mockImplementation(
        () =>
          new Promise<void>((resolve) => {
            complete = resolve;
          }),
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "添加" }));
    fireEvent.change(screen.getByLabelText("显示名称"), {
      target: { value: "Windows host" },
    });
    fireEvent.change(screen.getByLabelText("主机名"), {
      target: { value: "user@winbox" },
    });
    fireEvent.click(screen.getByRole("combobox", { name: "远端系统" }));
    fireEvent.click(screen.getByRole("option", { name: "Windows" }));
    const path = screen.getByLabelText("远程 Server 路径", {
      exact: false,
    }) as HTMLInputElement;
    expect(path.required).toBe(true);
    fireEvent.change(path, {
      target: { value: "C:\\工具 & '$()\\server.exe" },
    });
    fireEvent.submit(path.closest("form")!);
    await waitFor(() =>
      expect(save).toHaveBeenCalledWith(
        expect.objectContaining({
          platform: "windows",
          server_path: "C:\\工具 & '$()\\server.exe",
          ssh: "user@winbox",
        }),
        undefined,
      ),
    );
    expect(path.disabled).toBe(true);
    expect(
      (screen.getByRole("combobox", { name: "远端系统" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    complete();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("opens old records as Unix and preserves optional discovery", async () => {
    const save = setup([
      { id: "old", name: "Unix host", ssh: "devbox", server_path: null },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "编辑" }));
    expect(
      screen.getByRole("combobox", { name: "远端系统" }).textContent,
    ).toContain("Linux / macOS");
    const path = screen.getByLabelText("远程 Server 路径（可选）", {
      exact: false,
    }) as HTMLInputElement;
    expect(path.required).toBe(false);
    fireEvent.submit(path.closest("form")!);
    await waitFor(() =>
      expect(save).toHaveBeenCalledWith(
        expect.objectContaining({ platform: "unix", server_path: "" }),
        undefined,
      ),
    );
  });

  it("restores a saved Windows target when editing", () => {
    setup([
      {
        id: "win",
        name: "Windows host",
        ssh: "winbox",
        server_path: "C:/Tools/server.exe",
        platform: "windows",
      },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "编辑" }));
    expect(
      screen.getByRole("combobox", { name: "远端系统" }).textContent,
    ).toContain("Windows");
    expect(
      (
        screen.getByLabelText("远程 Server 路径", {
          exact: false,
        }) as HTMLInputElement
      ).value,
    ).toBe("C:/Tools/server.exe");
  });
});
