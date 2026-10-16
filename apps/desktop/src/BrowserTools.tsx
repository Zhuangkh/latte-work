import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronUp,
  MoreHorizontal,
  Minus,
  Plus,
  X,
} from "lucide-react";
import { message, native } from "./api";
import { Modal } from "./Modal";
import { openBrowser } from "./workspaceState";
import {
  clearBrowserHistory,
  removeBrowserVisit,
  useBrowserHistory,
} from "./browserHistory";

type Download = {
  id: string;
  name: string;
  url: string;
  status: string;
  path?: string;
  error?: string;
};
const labels: Record<string, string> = {
  downloading: "下载中",
  saving: "选择保存位置",
  completed: "已保存",
  cancelled: "已取消",
  failed: "失败",
  interrupted: "已中断",
};
export function BrowserTools({
  id,
  workspaceId,
  active,
  ready,
  url,
  enqueue,
  reportError,
}: {
  id: string;
  workspaceId: string;
  active: boolean;
  ready: boolean;
  url: string;
  enqueue: (work: () => Promise<unknown>) => Promise<unknown>;
  reportError: (error: string) => void;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [menu, setMenu] = useState(false);
  const [panel, setPanel] = useState<"history" | "downloads" | "clear" | null>(
    null,
  );
  const [find, setFind] = useState(false);
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<boolean | null>(null);
  const [zoom, setZoom] = useState(1);
  const zoomRef = useRef(1);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [notice, setNotice] = useState("");
  const [filter, setFilter] = useState("");
  const [cookies, setCookies] = useState(true);
  const [history, setHistory] = useState(true);
  const [downloads, setDownloads] = useState<Download[]>([]);
  const [clearDownloads, setClearDownloads] = useState(false);
  const report = useCallback(
    (error: unknown) => reportError(message(error)),
    [reportError],
  );
  const visits = useBrowserHistory(report);
  const state = useRef({ active, ready, url, panel });
  state.current = { active, ready, url, panel };
  const run = useRef<(action: string) => void>(() => {});
  const closeMenu = useCallback((restore = true) => {
    setMenu(false);
    if (restore) trigger.current?.focus();
  }, []);
  useEffect(() => {
    const unavailable = (event: Event) => {
      if ((event as CustomEvent<string>).detail !== id) return;
      closeMenu();
      setPanel(null);
    };
    window.addEventListener("browser-overlay-unavailable", unavailable);
    return () =>
      window.removeEventListener("browser-overlay-unavailable", unavailable);
  }, [id, closeMenu]);
  useEffect(() => {
    if (!active) {
      setMenu(false);
      setPanel(null);
    }
  }, [active]);
  useEffect(() => {
    setFound(null);
  }, [url, query]);
  useEffect(() => {
    if (find && active) search.current?.focus();
  }, [find, active]);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const listeners = [
      listen<string>("browser-shortcut", ({ payload }) => {
        const box = trigger.current?.getBoundingClientRect();
        if (
          !disposed &&
          state.current.active &&
          state.current.ready &&
          box &&
          box.width > 0 &&
          box.height > 0 &&
          !document.querySelector(
            '[role="dialog"], [role="menu"], [role="listbox"]',
          )
        )
          run.current(payload);
      }),
      listen<Download[]>("browser-downloads", ({ payload }) => {
        if (!disposed) setDownloads(payload);
      }),
      listen<string>("browser-download-error", ({ payload }) => {
        if (!disposed && state.current.active) reportError(payload);
      }),
    ];
    void invoke<Download[]>("browser_downloads").then((items) => {
      if (!disposed) setDownloads(items);
    }, report);
    return () => {
      disposed = true;
      listeners.forEach(
        (pending) =>
          void pending.then((unlisten) => unlisten()).catch(() => {}),
      );
    };
  }, [report, reportError]);
  async function perform(work: () => Promise<unknown>, success?: string) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setNotice("");
    try {
      const result = await enqueue(work);
      if (success) setNotice(success);
      reportError("");
      return result;
    } catch (error) {
      report(error);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }
  async function changeZoom(value: number) {
    const scale = Math.min(3, Math.max(0.25, Math.round(value * 100) / 100));
    await perform(async () => {
      await invoke("browser_action", { id, action: "zoom", zoom: scale });
      zoomRef.current = scale;
      setZoom(scale);
    });
  }
  async function findNext(backwards: boolean) {
    if (!query.trim()) return;
    const current = query;
    await perform(async () => {
      const result = await invoke<boolean>("browser_find", {
        id,
        query: current,
        backwards,
      });
      setFound(result);
    });
  }
  function choose(action: string) {
    closeMenu();
    if (action === "find") {
      setFind(true);
      search.current?.focus();
    } else if (
      action === "history" ||
      action === "downloads" ||
      action === "clear"
    ) {
      setFilter("");
      setPanel(action);
    } else if (action.startsWith("zoom-"))
      void changeZoom(
        action === "zoom-reset"
          ? 1
          : zoomRef.current + (action === "zoom-in" ? 0.1 : -0.1),
      );
    else if (action === "print" || action === "capture") {
      // Let the menu disappear and geometry restore the native surface first.
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          void perform(async () => {
            if (action === "print")
              await invoke("browser_action", { id, action: "print" });
            else {
              const path = await invoke<string | null>("browser_capture", {
                id,
              });
              if (path) setNotice("网页截图已保存");
            }
          });
        }),
      );
    }
  }
  run.current = choose;
  async function clearData() {
    await perform(async () => {
      if (cookies) await invoke("browser_clear_data", { id });
      if (history) clearBrowserHistory();
      if (clearDownloads) await invoke("browser_clear_downloads");
      setPanel(null);
      setNotice("浏览数据已清除");
    });
  }
  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="icon-button"
        aria-label="浏览器菜单"
        aria-haspopup="menu"
        aria-expanded={menu}
        disabled={!ready || busy}
        onClick={() => {
          if (menu) closeMenu();
          else {
            trigger.current?.focus();
            setMenu(true);
          }
        }}
      >
        <MoreHorizontal size={16} />
      </button>
      {menu && trigger.current && (
        <BrowserMenu
          trigger={trigger.current}
          close={closeMenu}
          zoom={zoom}
          busy={busy}
          hasPage={!!url}
          choose={choose}
          changeZoom={changeZoom}
        />
      )}
      {find &&
        active &&
        createPortal(
          <form
            className="browser-find"
            onSubmit={(event) => {
              event.preventDefault();
              // Portals keep React ancestry: do not submit the address form.
              event.stopPropagation();
              void findNext(false);
            }}
          >
            <input
              ref={search}
              aria-label="在页面中查找"
              value={query}
              maxLength={512}
              placeholder="在页面中查找"
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setFind(false);
                  trigger.current?.focus();
                } else if (event.key === "Enter" && event.shiftKey) {
                  event.preventDefault();
                  void findNext(true);
                }
              }}
            />
            <span role="status">
              {found === null ? "" : found ? "已找到" : "未找到"}
            </span>
            <button
              type="button"
              className="icon-button"
              aria-label="上一个匹配"
              disabled={busy || !query}
              onClick={() => void findNext(true)}
            >
              <ChevronUp size={16} />
            </button>
            <button
              type="submit"
              className="icon-button"
              aria-label="下一个匹配"
              disabled={busy || !query}
            >
              <ChevronDown size={16} />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label="关闭查找"
              onClick={() => {
                setFind(false);
                trigger.current?.focus();
              }}
            >
              <X size={16} />
            </button>
          </form>,
          trigger.current!.closest(".browser-pane")!,
        )}
      {notice &&
        active &&
        createPortal(
          <div className="browser-notice" role="status">
            {notice}
          </div>,
          trigger.current!.closest(".browser-pane")!,
        )}
      {panel &&
        active &&
        createPortal(
          <div
            className="modal-backdrop"
            onMouseDown={(event) => {
              if (event.target === event.currentTarget && !busy) setPanel(null);
            }}
          >
            <Modal
              className="modal browser-library"
              aria-label={
                panel === "history"
                  ? "浏览历史记录"
                  : panel === "downloads"
                    ? "浏览器下载"
                    : "清除浏览数据"
              }
              onKeyDown={(event) => {
                if (event.key === "Escape" && !busy) setPanel(null);
              }}
            >
              <button
                className="modal-close icon-button"
                aria-label="关闭"
                disabled={busy}
                onClick={() => setPanel(null)}
              >
                <X size={18} />
              </button>
              <h2>
                {panel === "history"
                  ? "历史记录"
                  : panel === "downloads"
                    ? "下载"
                    : "清除浏览数据"}
              </h2>
              {panel === "history" && (
                <>
                  <input
                    aria-label="搜索浏览记录"
                    value={filter}
                    placeholder="搜索标题或网址"
                    onChange={(event) => setFilter(event.target.value)}
                  />
                  {visits.length === 0 && (
                    <p className="muted">暂无浏览记录。</p>
                  )}
                  {visits.length > 0 &&
                    !visits.some((item) =>
                      (item.title + item.url)
                        .toLowerCase()
                        .includes(filter.toLowerCase()),
                    ) && <p className="muted">没有匹配的记录。</p>}
                  <div className="browser-records">
                    {[...visits]
                      .reverse()
                      .filter((item) =>
                        (item.title + item.url)
                          .toLowerCase()
                          .includes(filter.toLowerCase()),
                      )
                      .map((item) => (
                        <div className="browser-record" key={item.id}>
                          <button
                            className="browser-record-open"
                            onClick={() => {
                              try {
                                openBrowser(workspaceId, item.url, true);
                                setPanel(null);
                              } catch (error) {
                                report(error);
                              }
                            }}
                          >
                            <strong>{item.title}</strong>
                            <span>{item.url}</span>
                            <small>{new Date(item.at).toLocaleString()}</small>
                          </button>
                          <button
                            className="icon-button"
                            aria-label={`删除记录 ${item.title}`}
                            onClick={() => {
                              try {
                                removeBrowserVisit(item.id);
                              } catch (error) {
                                report(error);
                              }
                            }}
                          >
                            <X size={16} />
                          </button>
                        </div>
                      ))}
                  </div>
                  {!!visits.length && (
                    <button
                      onClick={() => {
                        setHistory(true);
                        setCookies(false);
                        setClearDownloads(false);
                        setPanel("clear");
                      }}
                    >
                      清空浏览记录…
                    </button>
                  )}
                </>
              )}
              {panel === "downloads" && (
                <>
                  {!downloads.length && <p className="muted">暂无下载记录。</p>}
                  <div className="browser-records">
                    {[...downloads].reverse().map((item) => (
                      <div className="browser-record" key={item.id}>
                        <div>
                          <strong>{item.name}</strong>
                          <span>{item.path || item.url}</span>
                          <small>
                            {item.error || labels[item.status] || item.status}
                          </small>
                        </div>
                        {item.status === "completed" && (
                          <button
                            disabled={busy}
                            onClick={() =>
                              void perform(() =>
                                invoke("browser_reveal_download", {
                                  id: item.id,
                                }),
                              )
                            }
                          >
                            显示位置
                          </button>
                        )}
                      </div>
                    ))}
                  </div>
                  {!!downloads.length && (
                    <button
                      disabled={busy}
                      onClick={() =>
                        void perform(() => invoke("browser_clear_downloads"))
                      }
                    >
                      清除已结束的记录
                    </button>
                  )}
                </>
              )}
              {panel === "clear" && (
                <>
                  <p>网站数据清除后需要重新登录。工作台设置和对话会保留。</p>
                  <label className="browser-check">
                    <input
                      type="checkbox"
                      checked={cookies}
                      onChange={(event) => setCookies(event.target.checked)}
                      disabled={busy}
                    />
                    Cookie、缓存和网站存储
                  </label>
                  <label className="browser-check">
                    <input
                      type="checkbox"
                      checked={history}
                      onChange={(event) => setHistory(event.target.checked)}
                      disabled={busy}
                    />
                    浏览历史记录
                  </label>
                  <label className="browser-check">
                    <input
                      type="checkbox"
                      checked={clearDownloads}
                      onChange={(event) =>
                        setClearDownloads(event.target.checked)
                      }
                      disabled={busy}
                    />
                    已结束的下载记录（保留文件）
                  </label>
                  <div className="browser-dialog-actions">
                    <button disabled={busy} onClick={() => setPanel(null)}>
                      取消
                    </button>
                    <button
                      className="primary"
                      disabled={
                        busy || (!cookies && !history && !clearDownloads)
                      }
                      onClick={() => void clearData()}
                    >
                      {busy ? "正在清除…" : "清除"}
                    </button>
                  </div>
                </>
              )}
            </Modal>
          </div>,
          document.body,
        )}
    </>
  );
}
function BrowserMenu({
  trigger,
  close,
  choose,
  zoom,
  hasPage,
  busy,
  changeZoom,
}: {
  trigger: HTMLButtonElement;
  close: (restore?: boolean) => void;
  choose: (action: string) => void;
  zoom: number;
  hasPage: boolean;
  busy: boolean;
  changeZoom: (zoom: number) => Promise<void>;
}) {
  const menu = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  useLayoutEffect(() => {
    const anchor = trigger.getBoundingClientRect(),
      box = menu.current!.getBoundingClientRect();
    setPosition({
      x: Math.max(
        8,
        Math.min(anchor.right - box.width, window.innerWidth - box.width - 8),
      ),
      y: Math.max(
        8,
        Math.min(anchor.bottom + 6, window.innerHeight - box.height - 8),
      ),
    });
    menu.current
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
  }, [trigger]);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (
        !menu.current?.contains(event.target as Node) &&
        !trigger.contains(event.target as Node)
      )
        close(false);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        close();
      } else if (event.key === "Tab") close(false);
      else if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        event.stopImmediatePropagation();
        const items = [
          ...menu.current!.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ),
        ];
        const index = items.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        items[
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? items.length - 1
              : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) %
                items.length
        ]?.focus();
      }
    };
    const resize = () => close(false);
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", resize);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", resize);
    };
  }, [trigger, close]);
  return createPortal(
    <div
      ref={menu}
      role="menu"
      aria-label="浏览器操作"
      className="project-context-menu browser-menu"
      style={{ left: position.x, top: position.y }}
    >
      <button
        role="menuitem"
        disabled={!hasPage || busy}
        onClick={() => choose("find")}
      >
        在页面中查找 <small>⌘ F</small>
      </button>
      <button
        role="menuitem"
        disabled={!hasPage || busy}
        onClick={() => choose("print")}
      >
        打印／另存 PDF… <small>⌘ P</small>
      </button>
      <div role="separator" />
      <div className="browser-zoom">
        <span>缩放</span>
        <button
          role="menuitem"
          aria-label="缩小网页"
          disabled={busy || zoom <= 0.25}
          onClick={() => void changeZoom(zoom - 0.1)}
        >
          <Minus size={16} />
        </button>
        <button
          role="menuitem"
          aria-label="重置网页缩放"
          disabled={busy}
          onClick={() => void changeZoom(1)}
        >
          {Math.round(zoom * 100)}%
        </button>
        <button
          role="menuitem"
          aria-label="放大网页"
          disabled={busy || zoom >= 3}
          onClick={() => void changeZoom(zoom + 0.1)}
        >
          <Plus size={16} />
        </button>
      </div>
      <div role="separator" />
      <button
        role="menuitem"
        disabled={!hasPage || busy}
        onClick={() => choose("capture")}
      >
        截取网页截图…
      </button>
      <button role="menuitem" onClick={() => choose("downloads")}>
        下载
      </button>
      <button role="menuitem" onClick={() => choose("history")}>
        历史记录
      </button>
      <div role="separator" />
      <button role="menuitem" onClick={() => choose("clear")}>
        清除浏览数据…
      </button>
    </div>,
    document.body,
  );
}
