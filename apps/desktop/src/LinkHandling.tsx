import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Copy, ExternalLink, Globe2 } from "lucide-react";
import { openBrowser } from "./workspaceState";
import { readLinkTarget, webLink, type LinkTarget } from "./browserLinks";
import { copyText } from "./clipboard";
import { message, native } from "./api";
import { useStatusIssues } from "./statusNotices";
const LinkContext = createContext<
  ((url: string, target?: LinkTarget) => void) | null
>(null);
export function LinkHandling({
  workspaceId,
  children,
}: {
  workspaceId: string;
  children: ReactNode;
}) {
  const [error, setError] = useState("");
  useStatusIssues([
    {
      id: "browser-links",
      title: "链接操作未完成",
      error,
      action: { label: "知道了", run: () => setError("") },
    },
  ]);
  function open(url: string, choice?: LinkTarget) {
    const target = choice ?? readLinkTarget();
    if (target === "internal") {
      try {
        openBrowser(workspaceId, url, choice === "internal");
      } catch (e) {
        setError(message(e));
      }
      return;
    }
    if (!native) {
      setError("系统浏览器仅在桌面应用内可用");
      return;
    }
    void invoke("open_external_url", { url }).catch((e) =>
      setError(message(e)),
    );
  }
  return <LinkContext.Provider value={open}>{children}</LinkContext.Provider>;
}
export function WebLink({
  href,
  children,
}: {
  href?: string;
  children?: ReactNode;
}) {
  const open = useContext(LinkContext);
  const link = useRef<HTMLAnchorElement>(null);
  const url = href ? webLink(href) : null;
  const [point, setPoint] = useState<{ x: number; y: number } | null>(null);
  const [error, setError] = useState("");
  if (!url)
    return <a href={href?.startsWith("#") ? href : undefined}>{children}</a>;
  return (
    <>
      <a
        ref={link}
        href={url}
        title={url}
        onClick={(event) => {
          event.preventDefault();
          if (open)
            open(url, event.metaKey || event.ctrlKey ? "internal" : undefined);
        }}
        onAuxClick={(event) => {
          if (event.button === 1) {
            event.preventDefault();
            open?.(url, "internal");
          }
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          setPoint({ x: event.clientX, y: event.clientY });
        }}
        onKeyDown={(event) => {
          if (
            event.key === "ContextMenu" ||
            (event.shiftKey && event.key === "F10")
          ) {
            event.preventDefault();
            const box = event.currentTarget.getBoundingClientRect();
            setPoint({ x: box.left, y: box.bottom });
          }
        }}
      >
        {children}
      </a>
      {point && (
        <LinkMenu
          point={point}
          close={() => {
            setPoint(null);
            link.current?.focus();
          }}
          choose={(action) => {
            setPoint(null);
            if (action === "copy")
              void copyText(url).catch((e) => setError(message(e)));
            else open?.(url, action);
          }}
        />
      )}
      {error && <span role="alert">复制失败：{error}</span>}
    </>
  );
}
function LinkMenu({
  point,
  close,
  choose,
}: {
  point: { x: number; y: number };
  close: () => void;
  choose: (action: LinkTarget | "copy") => void;
}) {
  const menu = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState(point);
  useLayoutEffect(() => {
    const box = menu.current!.getBoundingClientRect();
    setPosition({
      x: Math.max(8, Math.min(point.x, window.innerWidth - box.width - 8)),
      y: Math.max(8, Math.min(point.y, window.innerHeight - box.height - 8)),
    });
    menu.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [point]);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node)) close();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        close();
      } else if (event.key === "Tab") close();
      else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        const buttons = [
          ...menu.current!.querySelectorAll<HTMLButtonElement>("button"),
        ];
        const index = buttons.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        buttons[
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? buttons.length - 1
              : (index +
                  (event.key === "ArrowDown" ? 1 : -1) +
                  buttons.length) %
                buttons.length
        ]?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [close]);
  return createPortal(
    <div
      ref={menu}
      role="menu"
      aria-label="链接操作"
      className="project-context-menu link-context-menu"
      style={{ left: position.x, top: position.y }}
    >
      <button role="menuitem" onClick={() => choose("internal")}>
        <Globe2 size={16} />
        在内置新标签页中打开
      </button>
      <button role="menuitem" onClick={() => choose("system")}>
        <ExternalLink size={16} />
        在系统浏览器中打开
      </button>
      <div role="separator" />
      <button role="menuitem" onClick={() => choose("copy")}>
        <Copy size={16} />
        复制链接
      </button>
    </div>,
    document.body,
  );
}
