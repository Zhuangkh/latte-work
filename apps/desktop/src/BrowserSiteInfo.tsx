import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowLeft,
  ChevronRight,
  Cookie,
  Database,
  Info,
  LockKeyhole,
  ShieldAlert,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { SiteIcon } from "./SiteIcon";
import { webLink } from "./browserLinks";
import { Modal } from "./Modal";
import { message } from "./api";

export type SiteInfo = {
  url: string;
  title: string;
  connection: "secure" | "insecure" | "mixed" | "unknown";
  icon?: string;
  certificate?: { subject?: string; issuer?: string };
};
type SiteData = {
  url: string;
  token: string;
  domains: string[];
  types: string[];
};
export function BrowserSiteInfo({
  id,
  url,
  active,
  ready,
}: {
  id: string;
  url: string;
  active: boolean;
  ready: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<"main" | "connection" | "data">("main");
  const [info, setInfo] = useState<SiteInfo | null>(null);
  const [data, setData] = useState<SiteData | null>(null);
  const [error, setError] = useState("");
  const [dataError, setDataError] = useState("");
  const [dataRevision, refreshData] = useState(0);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const confirming = useRef(false);
  confirming.current = confirm;
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const address = webLink(url);
  const host = address ? new URL(address).host : "";
  useEffect(() => {
    const unavailable = (event: Event) => {
      if ((event as CustomEvent<string>).detail === id) setOpen(false);
    };
    window.addEventListener("browser-overlay-unavailable", unavailable);
    return () =>
      window.removeEventListener("browser-overlay-unavailable", unavailable);
  }, [id]);
  useEffect(() => {
    if (!active) setOpen(false);
  }, [active]);
  useEffect(() => {
    if (!open || !active) return;
    let disposed = false;
    setInfo(null);
    setError("");
    setPage("main");
    setData(null);
    setConfirm(false);
    void invoke<SiteInfo>("browser_site_info", { id, icon: false }).then(
      (value) => {
        if (!disposed && value && webLink(value.url) === address)
          setInfo(value);
        else if (!disposed) setError("页面已变化，请重新查看站点信息");
      },
      () => {
        if (!disposed) setError("暂时无法读取站点信息");
      },
    );
    panel.current?.focus();
    const outside = (event: PointerEvent) => {
      if (
        !confirming.current &&
        !busyRef.current &&
        !panel.current?.contains(event.target as Node) &&
        !trigger.current?.contains(event.target as Node)
      )
        setOpen(false);
    };
    const key = (event: KeyboardEvent) => {
      if (confirming.current) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (!busyRef.current) setOpen(false);
      }
      if (event.key === "Tab") {
        const buttons = [
          ...(panel.current?.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ) ?? []),
        ];
        const index = buttons.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        event.preventDefault();
        buttons[
          (index + (event.shiftKey ? -1 : 1) + buttons.length) % buttons.length
        ]?.focus();
      }
    };
    const close = () => {
      if (!busyRef.current) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", close);
    return () => {
      disposed = true;
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", close);
      if (trigger.current?.isConnected && !trigger.current.closest("[hidden]"))
        trigger.current.focus();
    };
  }, [open, active, id, address]);
  useEffect(() => {
    if (!open || !active || page !== "data") return;
    let disposed = false;
    setData(null);
    setDataError("");
    void invoke<SiteData>("browser_site_data", {
      id,
      url: address,
      token: null,
    }).then(
      (value) => {
        if (!disposed && value && webLink(value.url) === address)
          setData(value);
        else if (!disposed) setDataError("页面已变化，请重新查看网站数据");
      },
      (error) => {
        if (!disposed) setDataError(message(error));
      },
    );
    return () => {
      disposed = true;
    };
  }, [open, active, page, id, address, dataRevision]);
  async function clearData() {
    if (!data || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      const result = await invoke<{
        cleared?: boolean;
        reloadFailed?: boolean;
      }>("browser_site_data", { id, url: address, token: data.token });
      if (!result?.cleared)
        throw new Error("网站数据未能清除，请重新读取后重试");
      setConfirm(false);
      if (result.reloadFailed) {
        setData(null);
        setDataError("网站数据已清除，请手动刷新页面");
      } else setOpen(false);
    } catch (error) {
      setConfirm(false);
      setData(null);
      setDataError(message(error));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }
  const rect = trigger.current?.getBoundingClientRect();
  const connection = info?.connection;
  const ConnectionIcon =
    connection === "secure"
      ? LockKeyhole
      : connection === "unknown" || !connection
        ? Info
        : ShieldAlert;
  const status =
    error ||
    (!info
      ? "正在读取站点信息…"
      : connection === "secure"
        ? "连接是安全的"
        : connection === "insecure"
          ? "连接未加密"
          : connection === "mixed"
            ? "此页面包含不安全内容"
            : "连接信息暂不可用");
  const categories = data
    ? ([
        data.types.some((type) => /Cookies/i.test(type)) && "Cookie",
        data.types.some((type) => /Storage|Databases|FileSystem/i.test(type)) &&
          "本地存储",
        data.types.some((type) => /Cache|ServiceWorker/i.test(type)) && "缓存",
        data.types.some(
          (type) =>
            !/Cookies|Storage|Databases|FileSystem|Cache|ServiceWorker/i.test(
              type,
            ),
        ) && "其他网站数据",
      ].filter(Boolean) as string[])
    : [];
  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="icon-button browser-site-trigger"
        title="站点信息"
        aria-label="站点信息"
        aria-haspopup="dialog"
        aria-expanded={open && active}
        disabled={!ready || !address}
        onClick={() => setOpen((value) => !value)}
      >
        <SlidersHorizontal size={16} />
      </button>
      {open &&
        active &&
        address &&
        rect &&
        createPortal(
          <>
            <div
              ref={panel}
              className="browser-site-info"
              role="dialog"
              aria-label="站点信息"
              tabIndex={-1}
              style={{
                left: Math.max(
                  8,
                  Math.min(
                    rect.left,
                    window.innerWidth -
                      Math.min(344, window.innerWidth - 16) -
                      8,
                  ),
                ),
                top: Math.max(
                  8,
                  Math.min(rect.bottom + 8, window.innerHeight - 300),
                ),
              }}
            >
              <header>
                {page === "main" ? (
                  <SiteIcon url={url} />
                ) : (
                  <button
                    type="button"
                    className="icon-button"
                    aria-label="返回站点信息"
                    disabled={busy}
                    onClick={() => setPage("main")}
                  >
                    <ArrowLeft size={16} />
                  </button>
                )}
                <strong title={host}>
                  {page === "main"
                    ? host
                    : page === "connection"
                      ? "连接详情"
                      : "Cookie 和网站数据"}
                </strong>
                <button
                  type="button"
                  className="icon-button"
                  aria-label="关闭站点信息"
                  disabled={busy}
                  onClick={() => setOpen(false)}
                >
                  <X size={16} />
                </button>
              </header>
              {(page !== "main" || info?.title) && (
                <p
                  className="browser-site-title"
                  title={page === "main" ? info?.title : host}
                >
                  {page === "main" ? info?.title : host}
                </p>
              )}
              {page === "main" ? (
                <div className="browser-site-rows">
                  <button
                    type="button"
                    className="browser-site-row"
                    disabled={!info || !!error}
                    onClick={() => setPage("connection")}
                  >
                    <ConnectionIcon size={18} />
                    <span>{status}</span>
                    <ChevronRight size={16} />
                  </button>
                  <button
                    type="button"
                    className="browser-site-row"
                    onClick={() => setPage("data")}
                  >
                    <Cookie size={18} />
                    <span>Cookie 和网站数据</span>
                    <ChevronRight size={16} />
                  </button>
                </div>
              ) : page === "connection" ? (
                <>
                  <div className="browser-site-connection">
                    <ConnectionIcon size={18} />
                    <div>
                      <span>{status}</span>
                      {connection === "secure" && (
                        <small>此页面使用 HTTPS，证书已通过系统验证。</small>
                      )}
                      {connection === "insecure" && (
                        <small>此页面使用 HTTP，请勿输入密码等敏感信息。</small>
                      )}
                      {connection === "mixed" && (
                        <small>部分资源未加密，请谨慎输入敏感信息。</small>
                      )}
                    </div>
                  </div>
                  <dl className="browser-site-details">
                    <dt>地址</dt>
                    <dd>{host}</dd>
                    <dt>协议</dt>
                    <dd>
                      {new URL(address).protocol === "https:"
                        ? "HTTPS"
                        : "HTTP"}
                    </dd>
                    {info?.certificate?.subject && (
                      <>
                        <dt>证书</dt>
                        <dd>{info.certificate.subject}</dd>
                      </>
                    )}
                    {info?.certificate?.issuer && (
                      <>
                        <dt>颁发者</dt>
                        <dd>{info.certificate.issuer}</dd>
                      </>
                    )}
                  </dl>
                </>
              ) : (
                <div className="browser-site-data">
                  {dataError ? (
                    <>
                      <p role="status">{dataError}</p>
                      <button
                        type="button"
                        onClick={() => refreshData((value) => value + 1)}
                      >
                        重新读取
                      </button>
                    </>
                  ) : !data ? (
                    <p role="status">正在读取网站数据…</p>
                  ) : (
                    <>
                      {categories.length ? (
                        <>
                          <ul>
                            {categories.map((label) => (
                              <li key={label}>
                                {label === "Cookie" ? (
                                  <Cookie size={16} />
                                ) : (
                                  <Database size={16} />
                                )}
                                <span>{label}</span>
                              </li>
                            ))}
                          </ul>
                          <p className="muted">
                            数据范围：{data.domains.join("、")}
                          </p>
                          <button
                            type="button"
                            className="browser-site-clear"
                            onClick={() => setConfirm(true)}
                          >
                            清除网站数据…
                          </button>
                        </>
                      ) : (
                        <p className="muted">此站点暂未保存网站数据</p>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
            {confirm && data && (
              <div
                className="modal-backdrop browser-site-confirm-backdrop"
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.stopPropagation();
                    if (!busy) setConfirm(false);
                  }
                }}
              >
                <Modal
                  className="modal browser-site-confirm"
                  aria-label="清除网站数据"
                  onPointerDown={(event) => event.stopPropagation()}
                >
                  <h2>清除网站数据？</h2>
                  <p>
                    将清除 {data.domains.join("、")} 的 Cookie
                    和网站数据，可能退出登录，并刷新此页面。该域名下的子站点也会受到影响。
                  </p>
                  <footer>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => setConfirm(false)}
                    >
                      取消
                    </button>
                    <button
                      type="button"
                      className="primary"
                      disabled={busy}
                      onClick={() => void clearData()}
                    >
                      {busy ? "正在清除…" : "清除并刷新"}
                    </button>
                  </footer>
                </Modal>
              </div>
            )}
          </>,
          document.body,
        )}
    </>
  );
}
