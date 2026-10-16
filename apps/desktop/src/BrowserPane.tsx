import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, ArrowRight, ExternalLink, RotateCw } from "lucide-react";
import { native, message } from "./api";
import { useAppClose } from "./appLifecycle";
import { useStatusIssues } from "./statusNotices";
import {
  openBrowser,
  updateWorkspace,
  type WorkspaceTab,
} from "./workspaceState";
import { browserAddress, webLink } from "./browserLinks";
import { recordBrowserVisit, updateBrowserVisitTitle } from "./browserHistory";
import { BrowserTools } from "./BrowserTools";
import { BrowserSiteInfo, type SiteInfo } from "./BrowserSiteInfo";
import { rememberSiteIcon } from "./SiteIcon";
import { browserPreview } from "./browserPreview";
type BrowserTab = Extract<WorkspaceTab, { kind: "browser" }>;
export function BrowserPane({
  workspaceId,
  tab,
  active,
}: {
  workspaceId: string;
  tab: BrowserTab;
  active: boolean;
}) {
  const nativeId = useRef(crypto.randomUUID()).current;
  const [visited, setVisited] = useState(active);
  useEffect(() => {
    if (active) setVisited(true);
  }, [active]);
  const surface = useRef<HTMLDivElement>(null);
  const [address, setAddress] = useState(tab.url);
  const pageTitle = useRef({ url: "", title: "" });
  const [error, setError] = useState("");
  const [ready, setReady] = useState(false);
  const [preview, setPreview] = useState<{
    page: string;
    image: string;
  } | null>(null);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const activeRef = useRef(active);
  activeRef.current = active;
  const quitting = useRef(false);
  const initial = useRef(tab.url);
  const [revision, retry] = useState(0);
  function enqueue(work: () => Promise<unknown>) {
    const result = queue.current.catch(() => undefined).then(work);
    queue.current = result;
    return result;
  }
  useStatusIssues([
    {
      id: `browser:${tab.id}`,
      title: "浏览器操作未完成",
      error: active ? error : "",
      action: {
        label: ready ? "知道了" : "重试",
        run: () => {
          setError("");
          if (!ready) retry((v) => v + 1);
        },
      },
    },
  ]);
  useEffect(() => setAddress(tab.url), [tab.url]);
  useAppClose("浏览器", async () => {
    quitting.current = true;
    if (native) await enqueue(() => invoke("browser_close", { id: nativeId }));
  });
  useEffect(() => {
    if (!visited) return;
    if (!native) {
      setError("内置浏览器仅在桌面应用内可用");
      return;
    }
    let disposed = false;
    let metadataVersion = 0;
    let metadataUrl = initial.current;
    setReady(false);
    const listeners = [
      listen<{ id: string; url: string; title?: string; record?: boolean }>(
        "browser-location",
        ({ payload }) => {
          if (disposed || payload.id !== nativeId) return;
          const url = payload.url === "about:blank" ? "" : webLink(payload.url);
          if (url === null) return;
          if (metadataUrl !== url || payload.record === true) {
            metadataVersion++;
            metadataUrl = url;
          }
          const version = metadataVersion;
          if (url && payload.record === true) {
            void invoke<SiteInfo>("browser_site_info", {
              id: nativeId,
              icon: true,
            })
              .then((info) => {
                if (
                  !disposed &&
                  info &&
                  version === metadataVersion &&
                  webLink(info.url) === url
                )
                  rememberSiteIcon(url, info.icon);
              })
              .catch(() => undefined);
          }
          if (payload.title) pageTitle.current = { url, title: payload.title };
          if (url)
            try {
              if (payload.record !== false)
                recordBrowserVisit(
                  url,
                  payload.title ||
                    (pageTitle.current.url === url
                      ? pageTitle.current.title
                      : undefined),
                );
              else if (payload.title)
                updateBrowserVisitTitle(url, payload.title);
            } catch (error) {
              setError(message(error));
            }
          updateWorkspace(workspaceId, (state) => ({
            tabs: state.tabs.map((item) =>
              item.id === tab.id
                ? {
                    ...item,
                    url,
                    title:
                      payload.title ||
                      (item.kind === "browser" &&
                        item.url === url &&
                        item.title) ||
                      (url ? new URL(url).hostname : "浏览器"),
                  }
                : item,
            ),
          }));
        },
      ),
      listen<{ id: string; url: string }>("browser-new-tab", ({ payload }) => {
        if (!disposed && payload.id === nativeId)
          try {
            openBrowser(workspaceId, payload.url, true);
          } catch (e) {
            setError(message(e));
          }
      }),
    ];
    void enqueue(async () => {
      await Promise.all(listeners);
      if (disposed || quitting.current) return;
      // Create hidden, then the geometry effect exposes the selected surface.
      await invoke("browser_create", {
        id: nativeId,
        url: initial.current,
        bounds: { x: 0, y: 44, width: 1, height: 1 },
      });
      if (!disposed) setReady(true);
    }).catch((e) => {
      if (!disposed) setError(message(e));
    });
    return () => {
      disposed = true;
      listeners.forEach(
        (listener) =>
          void listener.then((unlisten) => unlisten()).catch(() => undefined),
      );
      void enqueue(() => invoke("browser_close", { id: nativeId })).catch(
        () => undefined,
      );
    };
  }, [nativeId, workspaceId, revision, visited]);
  useEffect(() => {
    if (!ready || !native) return;
    let disposed = false;
    let frame = 0;
    let last = "";
    type Bounds = { x: number; y: number; width: number; height: number };
    let latest: { bounds: Bounds | null; overlay: boolean } = {
      bounds: null,
      overlay: false,
    };
    let picture: Awaited<ReturnType<typeof browserPreview>> | null = null;
    const clearPicture = () => {
      setPreview(null);
      picture?.release();
      picture = null;
    };
    let sending = false;
    const send = async () => {
      if (sending) return;
      sending = true;
      try {
        let sent: string;
        do {
          const state = latest;
          sent = JSON.stringify(state);
          if (state.bounds && state.overlay && !picture) {
            try {
              const captured = (await enqueue(() =>
                browserPreview(nativeId),
              )) as Awaited<ReturnType<typeof browserPreview>>;
              if (disposed || !latest.bounds || !latest.overlay) {
                captured.release();
                continue;
              }
              picture = captured;
              // Commit decoded pixels before hiding the native surface.
              flushSync(() =>
                setPreview({ page: tab.url, image: captured.url }),
              );
            } catch (error) {
              if (disposed) return;
              // A failed capture must not replace a working page with a blank canvas.
              window.dispatchEvent(
                new CustomEvent("browser-overlay-unavailable", {
                  detail: nativeId,
                }),
              );
              setError(message(error));
              await enqueue(() =>
                invoke("browser_sync", { id: nativeId, bounds: latest.bounds }),
              );
              continue;
            }
          }
          if (disposed) return;
          const bounds = state.overlay ? null : state.bounds;
          await enqueue(() => invoke("browser_sync", { id: nativeId, bounds }));
          if (!state.overlay) clearPicture();
        } while (!disposed && sent !== JSON.stringify(latest));
      } catch (e) {
        if (!disposed) setError(message(e));
      } finally {
        sending = false;
      }
    };
    const sync = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (disposed || quitting.current) return;
        const box = surface.current?.getBoundingClientRect();
        const overlay =
          !!box &&
          [
            ...document.querySelectorAll<HTMLElement>(
              '[role="dialog"], [role="menu"], [role="listbox"], .status-center-popover, .task-overview-popover',
            ),
          ].some((element) => {
            const rect = element.getBoundingClientRect();
            return (
              rect.width > 0 &&
              rect.height > 0 &&
              rect.left < box.right &&
              rect.right > box.left &&
              rect.top < box.bottom &&
              rect.bottom > box.top
            );
          });
        const bounds =
          activeRef.current &&
          !!tab.url &&
          box &&
          box.width > 0 &&
          box.height > 0
            ? { x: box.x, y: box.y, width: box.width, height: box.height }
            : null;
        const state = { bounds, overlay: !!bounds && overlay };
        const signature = JSON.stringify(state);
        if (signature === last) return;
        last = signature;
        latest = state;
        void send();
      });
    };
    const resize = new ResizeObserver(sync);
    if (surface.current) resize.observe(surface.current);
    const mutations = new MutationObserver(sync);
    mutations.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["hidden", "class", "style"],
    });
    window.addEventListener("resize", sync);
    sync();
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      resize.disconnect();
      mutations.disconnect();
      window.removeEventListener("resize", sync);
      clearPicture();
      void enqueue(() =>
        invoke("browser_sync", { id: nativeId, bounds: null }),
      ).catch(() => undefined);
    };
  }, [active, ready, nativeId, tab.url]);
  function action(action: string, url?: string) {
    void enqueue(() =>
      invoke("browser_action", { id: nativeId, action, url }),
    ).then(
      () => setError(""),
      (e) => setError(message(e)),
    );
  }
  return (
    <div className="browser-pane">
      <form
        className="browser-toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          const url = browserAddress(address);
          if (!url) {
            setError("请输入有效域名或 HTTP / HTTPS 网页地址");
            return;
          }
          setAddress(url);
          action("navigate", url);
        }}
      >
        <button
          type="button"
          className="icon-button"
          aria-label="后退"
          title="后退"
          disabled={!ready}
          onClick={() => action("back")}
        >
          <ArrowLeft size={16} />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label="前进"
          title="前进"
          disabled={!ready}
          onClick={() => action("forward")}
        >
          <ArrowRight size={16} />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label="刷新网页"
          title="刷新"
          disabled={!ready || !tab.url}
          onClick={() => action("reload")}
        >
          <RotateCw size={16} />
        </button>
        <div className="browser-address">
          <BrowserSiteInfo
            key={tab.url}
            id={nativeId}
            url={tab.url}
            active={active}
            ready={ready}
          />
          <input
            aria-label="网页地址"
            value={address}
            placeholder="输入域名或网页地址"
            spellCheck={false}
            onChange={(event) => setAddress(event.target.value)}
            disabled={!ready}
          />
        </div>
        <button
          type="button"
          className="icon-button"
          aria-label="在系统浏览器中打开"
          title="在系统浏览器中打开"
          disabled={!tab.url}
          onClick={() =>
            void invoke("open_external_url", { url: tab.url }).catch((e) =>
              setError(message(e)),
            )
          }
        >
          <ExternalLink size={16} />
        </button>
        <BrowserTools
          id={nativeId}
          workspaceId={workspaceId}
          active={active}
          ready={ready}
          url={tab.url}
          enqueue={enqueue}
          reportError={setError}
        />
      </form>
      <div ref={surface} className="browser-surface">
        {preview && active && preview.page === tab.url && (
          <img
            className="browser-page-preview"
            src={preview.image}
            alt=""
            aria-hidden="true"
          />
        )}
        {!tab.url && (
          <p className="muted">输入网页地址，或从对话中打开链接。</p>
        )}
        {!native && <p>请在 Latte Work 桌面应用中打开网页。</p>}
      </div>
    </div>
  );
}
