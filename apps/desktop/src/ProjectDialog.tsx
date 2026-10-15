import { Modal } from "./Modal";
import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Check,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderPlus,
  LoaderCircle,
  Monitor,
  Plus,
  Server,
  X,
} from "lucide-react";
import {
  chooseProjectFolder,
  connect,
  message,
  request,
  type Host,
} from "./api";
import type { PathSyntax, Response } from "./protocol";
import type { Workbench } from "./useWorkbench";
import { projectFolderName } from "./projectFolderName";

export function ProjectDialog({ state }: { state: Workbench }) {
  const {
    hosts,
    projectHostId,
    setProjectHostId,
    projectPath,
    setProjectPath,
    projectName,
    setProjectName,
    busy,
    error,
    setError,
    setModal,
    addProject,
  } = state;
  const host = hosts.find((h) => h.id === projectHostId) ?? hosts[0];
  const hostKey = JSON.stringify(host);
  const currentHost = useRef(hostKey);
  currentHost.current = hostKey;
  const [browsedHost, setBrowsedHost] = useState<{
    hostKey: string;
    pathSyntax?: PathSyntax;
  }>();
  const pathSyntax =
    state.hostPathSyntax(host.id) ??
    (browsedHost?.hostKey === hostKey ? browsedHost.pathSyntax : undefined);
  const pathExample = pathSyntax
    ? {
        windows: "C:\\Users\\you\\projects\\my-project",
        posix: "/home/you/projects/my-project",
      }[pathSyntax]
    : "输入主机上的绝对路径";
  const [selectedFolder, setSelectedFolder] = useState<{
    hostKey: string;
    path: string;
    name?: string;
  }>();
  const folderName =
    (selectedFolder?.hostKey === hostKey && selectedFolder.path === projectPath
      ? selectedFolder.name
      : undefined) ?? projectFolderName(projectPath, pathSyntax);
  const [menu, setMenu] = useState(false);
  const source = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (menu)
      source.current
        ?.querySelector<HTMLButtonElement>('[aria-checked="true"]')
        ?.focus();
  }, [menu]);
  const [picking, setPicking] = useState(false);
  const [remoteBrowser, setRemoteBrowser] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  function selectFolder(folder: { path: string; name?: string }) {
    setProjectPath(folder.path);
    setSelectedFolder({ ...folder, hostKey });
    const selectedName = folder.name;
    if (selectedName)
      setProjectName((name) => (name.trim() ? name : selectedName));
    setRemoteBrowser(false);
    setError("");
  }
  async function pickFolder() {
    setMenu(false);
    if (host.ssh) {
      setRemoteBrowser(true);
      return;
    }
    setPicking(true);
    try {
      const folder = await chooseProjectFolder();
      if (mounted.current && currentHost.current === hostKey && folder)
        selectFolder(folder);
    } catch (e) {
      if (mounted.current && currentHost.current === hostKey)
        setError(message(e));
    } finally {
      if (mounted.current) setPicking(false);
    }
  }
  const disabled = busy || picking;
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !disabled) setModal(null);
      }}
    >
      <Modal
        className="modal project-modal"
        role="dialog"
        aria-modal="true"
        aria-label="创建项目"
        onKeyDown={(event) => {
          if (event.key !== "Escape" || disabled) return;
          event.preventDefault();
          event.stopPropagation();
          if (menu) {
            setMenu(false);
            event.currentTarget
              .querySelector<HTMLButtonElement>(".source-trigger")
              ?.focus();
          } else if (remoteBrowser) setRemoteBrowser(false);
          else setModal(null);
        }}
      >
        <button
          className="modal-close icon-button"
          aria-label="关闭"
          disabled={disabled}
          onClick={() => setModal(null)}
        >
          <X size={20} />
        </button>
        {remoteBrowser ? (
          <RemoteFolderBrowser
            key={hostKey}
            host={host}
            initialPath={projectPath}
            onPathSyntax={(syntax) =>
              setBrowsedHost({ hostKey, pathSyntax: syntax })
            }
            cancel={() => setRemoteBrowser(false)}
            select={selectFolder}
          />
        ) : (
          <>
            <h2>创建项目</h2>
            <form onSubmit={(e) => void addProject(e)}>
              <fieldset disabled={disabled}>
                <div className="project-name-input">
                  <Folder size={21} />
                  <input
                    autoFocus
                    aria-label="项目名称"
                    placeholder="项目名称"
                    maxLength={100}
                    value={projectName}
                    onChange={(e) => setProjectName(e.target.value)}
                  />
                </div>
                <h3>源文件夹</h3>
                <div className="folder-source">
                  <div className="source-selector" ref={source}>
                    <button
                      type="button"
                      className="source-trigger"
                      aria-haspopup="menu"
                      aria-expanded={menu}
                      onClick={() => setMenu((v) => !v)}
                    >
                      在<strong>{host.ssh ? host.name : "此电脑"}</strong>
                      上添加文件夹
                      <ChevronDown size={17} />
                    </button>
                    {menu && (
                      <>
                        <button
                          type="button"
                          className="source-dismiss"
                          aria-label="收起主机选择"
                          onClick={() => setMenu(false)}
                        />
                        <div
                          className="source-menu"
                          role="menu"
                          aria-label="项目所在主机"
                          onKeyDown={(event) => {
                            if (!["ArrowDown", "ArrowUp"].includes(event.key))
                              return;
                            event.preventDefault();
                            const items = Array.from(
                              event.currentTarget.querySelectorAll<HTMLButtonElement>(
                                "button",
                              ),
                            );
                            const index = items.indexOf(
                              document.activeElement as HTMLButtonElement,
                            );
                            items[
                              (index +
                                (event.key === "ArrowDown" ? 1 : -1) +
                                items.length) %
                                items.length
                            ]?.focus();
                          }}
                        >
                          {hosts.map((h, i) => (
                            <div key={h.id}>
                              {i === 1 && (
                                <div className="source-menu-heading">
                                  远程设备
                                </div>
                              )}
                              <button
                                type="button"
                                role="menuitemradio"
                                aria-checked={h.id === projectHostId}
                                onClick={() => {
                                  if (h.id !== projectHostId) {
                                    setProjectHostId(h.id);
                                    setProjectPath("");
                                    setError("");
                                  }
                                  setMenu(false);
                                  source.current
                                    ?.querySelector<HTMLButtonElement>(
                                      ".source-trigger",
                                    )
                                    ?.focus();
                                }}
                              >
                                {h.ssh ? (
                                  <Server size={18} />
                                ) : (
                                  <Monitor size={18} />
                                )}
                                <span>{h.ssh ? h.name : "此电脑"}</span>
                                {h.id === projectHostId && <Check size={16} />}
                              </button>
                            </div>
                          ))}
                          <button
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              setError("");
                              state.setSettingsTab("ssh");
                              state.setSettingsReturnToProject(true);
                              setModal("settings");
                            }}
                          >
                            <Plus size={18} />
                            <span>添加远程</span>
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                  {projectPath && (
                    <div className="chosen-folder">
                      <Folder size={19} />
                      <div>
                        <strong>{folderName ?? "项目文件夹"}</strong>
                        <span title={projectPath}>{projectPath}</span>
                      </div>
                    </div>
                  )}
                  <button
                    type="button"
                    className="choose-folder"
                    onClick={() => void pickFolder()}
                  >
                    {picking ? (
                      <LoaderCircle className="spin" size={18} />
                    ) : (
                      <FolderPlus size={18} />
                    )}
                    {projectPath ? "更换文件夹" : "添加"}
                  </button>
                </div>
                <details className="manual-path">
                  <summary>输入文件夹路径</summary>
                  <input
                    aria-label="项目绝对路径"
                    placeholder={pathExample}
                    value={projectPath}
                    onChange={(e) => {
                      setSelectedFolder(undefined);
                      setProjectPath(e.target.value);
                    }}
                  />
                </details>
                {error && (
                  <div className="form-error" role="alert">
                    {error}
                  </div>
                )}
                <div className="modal-actions">
                  <button type="button" onClick={() => setModal(null)}>
                    取消
                  </button>
                  <button className="primary" disabled={!projectPath.trim()}>
                    {busy ? "正在创建…" : "创建项目"}
                  </button>
                </div>
              </fieldset>
            </form>
          </>
        )}
      </Modal>
    </div>
  );
}

function RemoteFolderBrowser({
  host,
  initialPath,
  onPathSyntax,
  cancel,
  select,
}: {
  host: Host;
  initialPath: string;
  onPathSyntax: (syntax: PathSyntax | undefined) => void;
  cancel: () => void;
  select: (folder: { path: string; name?: string }) => void;
}) {
  const [listing, setListing] =
    useState<Extract<Response, { kind: "directories" }>>();
  const [path, setPath] = useState(initialPath);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  async function browse(target: string | null, entryName?: string) {
    const ticket = ++generation.current;
    setLoading(true);
    setError("");
    try {
      const hello = await connect(host);
      if (hello.kind !== "hello")
        throw new Error(
          hello.kind === "error" ? hello.message : "主机连接失败",
        );
      if (ticket !== generation.current) return;
      onPathSyntax(hello.path_syntax);
      const response = await request(host.id, {
        method: "browse_directories",
        path: target,
      });
      if (ticket !== generation.current) return;
      if (response.kind !== "directories")
        throw new Error("此 Server 不支持文件夹浏览，请更新远程 Server");
      setListing({ ...response, name: response.name ?? entryName });
      setPath(response.path);
    } catch (e) {
      if (ticket === generation.current) setError(message(e));
    } finally {
      if (ticket === generation.current) setLoading(false);
    }
  }
  useEffect(() => {
    void browse(initialPath || null);
    return () => {
      generation.current++;
    };
  }, [host.id]);
  return (
    <>
      <h2>选择文件夹</h2>
      <p className="folder-browser-host">
        <Server size={15} />
        {host.name}
      </p>
      <form
        className="remote-path"
        onSubmit={(e) => {
          e.preventDefault();
          void browse(path.trim() || null);
        }}
      >
        <input
          aria-label="远程目录路径"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="输入绝对路径"
        />
        <button disabled={loading}>前往</button>
      </form>
      <div className="directory-list" aria-busy={loading}>
        <button
          disabled={loading || !listing?.parent}
          onClick={() => void browse(listing?.parent ?? null)}
        >
          <ArrowLeft size={16} />
          上一级
        </button>
        {loading ? (
          <div className="directory-empty">
            <LoaderCircle size={18} className="spin" />
            正在读取目录…
          </div>
        ) : (
          <>
            {listing?.entries.map((entry) => (
              <button
                key={entry.path}
                onClick={() => void browse(entry.path, entry.name)}
              >
                <Folder size={17} />
                <span>{entry.name}</span>
                <ChevronRight size={15} />
              </button>
            ))}
            {listing?.entries.length === 0 && (
              <div className="directory-empty">此目录没有子文件夹</div>
            )}
          </>
        )}
      </div>
      {listing?.truncated && (
        <p>目录较大，仅显示部分条目。可输入完整路径前往。</p>
      )}
      {error && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      <div className="modal-actions">
        <button onClick={cancel}>返回</button>
        <button
          className="primary"
          disabled={loading || !listing || !!error || path !== listing.path}
          onClick={() =>
            listing && select({ path: listing.path, name: listing.name })
          }
        >
          选择此文件夹
        </button>
      </div>
    </>
  );
}
