import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Search,
} from "lucide-react";
import { message, request } from "./api";
import type { GitCommit, GitReviewScope } from "./protocol";

export type ReviewScope = GitReviewScope | "lastTurn" | "turn" | "task";
function CommitChoices({
  hostId,
  projectId,
  selected,
  choose,
}: {
  hostId: string;
  projectId: string;
  selected: string;
  choose: (commit: GitCommit) => void;
}) {
  const [rows, setRows] = useState<GitCommit[]>([]);
  const [head, setHead] = useState<string | null>(null);
  const [offset, setOffset] = useState(0);
  const [more, setMore] = useState(false);
  const [pending, setPending] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [retry, setRetry] = useState(0);
  const cursor = useRef<{ head: string | null; offset: number }>({
    head: null,
    offset: 0,
  });
  useEffect(() => {
    let disposed = false;
    setPending(true);
    setError("");
    void request(hostId, {
      method: "git_commits",
      project_id: projectId,
      ...cursor.current,
    })
      .then((response) => {
        if (response.kind !== "git_commits")
          throw new Error("Server 不支持提交历史，请更新后重试");
        if (disposed) return;
        if (offset && (response.head !== head || !response.commits.length))
          throw new Error("提交历史游标没有前进，请重新打开菜单");
        setHead(response.head);
        setRows((previous) =>
          offset ? [...previous, ...response.commits] : response.commits,
        );
        setMore(response.has_more);
      })
      .catch((cause) => {
        if (!disposed) setError(message(cause));
      })
      .finally(() => {
        if (!disposed) setPending(false);
      });
    return () => {
      disposed = true;
    };
    // The ref is updated only when the user explicitly requests another page.
  }, [hostId, projectId, offset, retry]);
  const visible = rows.filter((row) =>
    `${row.subject} ${row.oid}`
      .toLowerCase()
      .includes(query.trim().toLowerCase()),
  );
  return (
    <div className="change-commit-choices">
      <label className="change-commit-search">
        <Search size={14} />
        <input
          aria-label="搜索提交"
          placeholder="搜索已加载的提交"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </label>
      <div className="change-commit-rows" role="listbox" aria-label="选择提交">
        {visible.map((row) => (
          <button
            key={row.oid}
            role="option"
            aria-selected={selected === row.oid}
            title={`${row.subject}\n${row.oid}`}
            onClick={() => choose(row)}
          >
            <span>
              <strong>{row.subject || "无标题提交"}</strong>
              <small>{row.oid.slice(0, 8)}</small>
            </span>
            {selected === row.oid && <Check size={16} />}
          </button>
        ))}
        {!pending && !error && !visible.length && (
          <p>{rows.length ? "没有匹配的提交" : "仓库尚无提交"}</p>
        )}
        {error && <p role="alert">{error}</p>}
        {pending && <p role="status">正在读取提交…</p>}
        {error ? (
          <button onClick={() => setRetry((v) => v + 1)}>重试</button>
        ) : (
          more && (
            <button
              disabled={pending}
              onClick={() => {
                cursor.current = { head, offset: rows.length };
                setOffset(rows.length);
              }}
            >
              加载更早的提交
            </button>
          )
        )}
      </div>
      <p className="change-commit-hint">当前分支主线上的提交</p>
    </div>
  );
}

export function ChangeScopePicker({
  hostId,
  projectId,
  scope,
  commit,
  sessionId,
  requestId,
  disabled,
  choose,
}: {
  hostId: string;
  projectId: string;
  scope: ReviewScope;
  commit?: GitCommit;
  sessionId?: string;
  requestId?: string;
  disabled: boolean;
  choose: (scope: ReviewScope, commit?: GitCommit) => void;
}) {
  const context = JSON.stringify([hostId, projectId, sessionId, requestId]);
  const [openedFor, setOpenedFor] = useState<string | null>(null);
  const open = openedFor === context && !disabled;
  const [commits, showCommits] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null),
    menu = useRef<HTMLDivElement>(null);
  const items: { value: ReviewScope; label: string; separator?: boolean }[] = [
    ...(requestId ? [{ value: "turn" as const, label: "本轮" }] : []),
    { value: "lastTurn", label: "上一轮" },
    ...(sessionId ? [{ value: "task" as const, label: "任务净变更" }] : []),
    { value: "worktree", label: "未提交", separator: true },
    { value: "unstaged", label: "未暂存" },
    { value: "staged", label: "已暂存" },
    { value: "commit", label: "已提交", separator: true },
    { value: "branch", label: "分支" },
  ];
  const close = useCallback((restore = true) => {
    setOpenedFor(null);
    showCommits(false);
    if (restore) trigger.current?.focus();
  }, []);
  useEffect(() => {
    close(false);
  }, [hostId, projectId, close]);
  useEffect(() => {
    if (disabled) close(false);
  }, [disabled, close]);
  useEffect(() => {
    if (!open) return;
    const root = menu.current!;
    root.querySelector<HTMLButtonElement>(`[data-scope="${scope}"]`)?.focus();
    const outside = (event: PointerEvent) => {
      if (
        !root.contains(event.target as Node) &&
        !trigger.current?.contains(event.target as Node)
      )
        close(false);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        close();
      } else if (event.key === "Tab") close(false);
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
  }, [open, scope, close]);
  useEffect(() => {
    if (commits)
      menu.current?.querySelector<HTMLInputElement>("input")?.focus();
  }, [commits]);
  const rect = trigger.current?.getBoundingClientRect();
  const narrow = window.innerWidth < 620;
  const label =
    scope === "commit"
      ? `已提交 · ${commit?.oid.slice(0, 8) ?? "选择提交"}`
      : items.find((item) => item.value === scope)?.label;
  return (
    <>
      <button
        ref={trigger}
        role="combobox"
        className="select-trigger change-scope-trigger"
        aria-label="变更范围"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        title={commit && scope === "commit" ? commit.subject : label}
        onClick={() => {
          if (open) close();
          else setOpenedFor(context);
        }}
      >
        <span>{label}</span>
        <ChevronDown size={14} />
      </button>
      {open &&
        rect &&
        createPortal(
          <div
            ref={menu}
            className={`project-context-menu change-scope-menu${commits ? " with-commits" : ""}${narrow ? " narrow" : ""}`}
            style={{
              left: Math.max(
                8,
                Math.min(
                  rect.left,
                  window.innerWidth - (commits && !narrow ? 528 : 248),
                ),
              ),
              top: Math.max(
                8,
                Math.min(rect.bottom + 6, window.innerHeight - 380),
              ),
              maxHeight: Math.max(
                100,
                window.innerHeight -
                  Math.max(
                    8,
                    Math.min(rect.bottom + 6, window.innerHeight - 380),
                  ) -
                  8,
              ),
            }}
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft" && commits) {
                event.preventDefault();
                showCommits(false);
                menu.current
                  ?.querySelector<HTMLButtonElement>('[data-scope="commit"]')
                  ?.focus();
              }
              if (
                event.key === "ArrowRight" &&
                (event.target as HTMLElement).dataset.scope === "commit"
              ) {
                event.preventDefault();
                showCommits(true);
              }
              if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
                if (
                  (event.target as HTMLElement).tagName === "INPUT" &&
                  !event.key.startsWith("Arrow")
                )
                  return;
                event.preventDefault();
                const target = event.target as HTMLElement;
                const list =
                  target.closest('[role="listbox"]') ??
                  target
                    .closest(".change-commit-choices")
                    ?.querySelector('[role="listbox"]') ??
                  menu.current;
                const buttons = Array.from(
                  list?.querySelectorAll<HTMLButtonElement>(
                    "button:not(:disabled)",
                  ) ?? [],
                );
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
            }}
          >
            {(!narrow || !commits) && (
              <div
                role="listbox"
                aria-label="变更范围"
                className="change-scope-items"
              >
                {items.map((item) => (
                  <Fragment key={item.value}>
                    {item.separator && <div role="separator" />}
                    <button
                      data-scope={item.value}
                      role="option"
                      aria-selected={scope === item.value}
                      aria-haspopup={
                        item.value === "commit" ? "listbox" : undefined
                      }
                      aria-expanded={
                        item.value === "commit" ? commits : undefined
                      }
                      onClick={() => {
                        if (item.value === "commit") showCommits((v) => !v);
                        else {
                          choose(item.value);
                          close();
                        }
                      }}
                    >
                      <span>{item.label}</span>
                      {scope === item.value && <Check size={16} />}
                      {item.value === "commit" && <ChevronRight size={16} />}
                    </button>
                  </Fragment>
                ))}
              </div>
            )}
            {commits && (
              <div className="change-commit-submenu">
                {narrow && (
                  <button
                    className="change-scope-back"
                    onClick={() => {
                      showCommits(false);
                    }}
                  >
                    <ChevronLeft size={16} />
                    变更范围
                  </button>
                )}
                <CommitChoices
                  hostId={hostId}
                  projectId={projectId}
                  selected={commit?.oid ?? ""}
                  choose={(row) => {
                    choose("commit", row);
                    close();
                  }}
                />
              </div>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
