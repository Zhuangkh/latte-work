import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ChevronDown,
  ChevronRight,
  Columns2,
  FileCode2,
  Folder,
  PanelRight,
  RefreshCw,
  Search,
  WrapText,
} from "lucide-react";
import type { GitReviewFile } from "./protocol";
import { message } from "./api";
import { ReviewDiff } from "./ReviewDiff";
import { useStatusIssues } from "./statusNotices";

type DiffContent = { text: string; truncated: boolean };
type Loader = (path: string) => Promise<DiffContent>;
const statusLabels: Record<string, string> = {
  A: "新增",
  D: "删除",
  M: "修改",
  R: "重命名",
  C: "复制",
  U: "冲突",
  T: "类型变化",
};
export function DiffStats({
  file,
}: {
  file: Pick<GitReviewFile, "added" | "removed" | "binary">;
}) {
  return file.binary ? (
    <small>二进制</small>
  ) : file.added === null || file.removed === null ? (
    <small>未统计</small>
  ) : (
    <span className="change-totals">
      <b>+{file.added}</b>
      <em>-{file.removed}</em>
    </span>
  );
}
/** Serialize admission to a small bounded set of independent visible-file reads. */
function diffQueue() {
  const pending: (() => void)[] = [];
  let running = 0;
  function next() {
    while (running < 3 && pending.length) pending.shift()!();
  }
  return function enqueue<T>(read: () => Promise<T>) {
    return new Promise<T>((resolve, reject) => {
      pending.push(() => {
        running++;
        void read()
          .then(resolve, reject)
          .finally(() => {
            running--;
            next();
          });
      });
      next();
    });
  };
}

type TreeNode = {
  path: string;
  name: string;
  file?: GitReviewFile;
  children: TreeNode[];
};
function fileTree(files: GitReviewFile[]) {
  const root: TreeNode = { path: "", name: "", children: [] };
  for (const file of files) {
    let parent = root;
    const parts = file.path.split("/");
    for (let i = 0; i < parts.length; i++) {
      const path = parts.slice(0, i + 1).join("/");
      let child = parent.children.find((n) => n.path === path);
      if (!child) {
        child = { path, name: parts[i], children: [] };
        parent.children.push(child);
      }
      if (i === parts.length - 1) child.file = file;
      parent = child;
    }
  }
  function compress(node: TreeNode): TreeNode {
    node.children = node.children
      .map(compress)
      .sort(
        (a, b) =>
          Number(!!a.file) - Number(!!b.file) || a.name.localeCompare(b.name),
      );
    if (!node.file && node.children.length === 1 && !node.children[0].file) {
      const child = node.children[0];
      return { ...child, name: `${node.name}/${child.name}` };
    }
    return node;
  }
  return root.children
    .map(compress)
    .sort(
      (a, b) =>
        Number(!!a.file) - Number(!!b.file) || a.name.localeCompare(b.name),
    );
}
function ReviewFileSection({
  file,
  active,
  initiallyVisible,
  selected,
  loadDiff,
  enqueue,
  split,
  wrap,
  register,
  onSelect,
  reveal,
}: {
  file: GitReviewFile;
  active: boolean;
  initiallyVisible: boolean;
  selected: boolean;
  loadDiff: Loader;
  enqueue: ReturnType<typeof diffQueue>;
  split: boolean;
  wrap: boolean;
  register: (path: string, element: HTMLElement | null) => void;
  onSelect: () => void;
  reveal: number;
}) {
  const issueId = useId();
  const root = useRef<HTMLElement>(null);
  const [visible, setVisible] = useState(initiallyVisible);
  const [near, setNear] = useState(initiallyVisible);
  const reservedHeight = useRef(80);
  const [collapsed, collapse] = useState(false);
  const [data, setData] = useState<{ loader: Loader; content: DiffContent }>();
  const content = data?.loader === loadDiff ? data.content : undefined;
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!active || !root.current || typeof IntersectionObserver === "undefined")
      return;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            setVisible(true);
            setNear(true);
          } else {
            const patch = root.current?.querySelector(".review-file-patch");
            if (patch)
              reservedHeight.current = Math.max(
                80,
                patch.getBoundingClientRect().height,
              );
            setNear(false);
          }
        }
      },
      { root: root.current.parentElement, rootMargin: "200px" },
    );
    observer.observe(root.current);
    return () => observer.disconnect();
  }, [active]);
  useEffect(() => {
    if (selected) {
      setVisible(true);
    }
  }, [selected]);
  useEffect(() => {
    if (reveal) {
      setVisible(true);
      setNear(true);
      collapse(false);
    }
  }, [reveal]);
  useEffect(() => {
    if (!active || !visible || collapsed || content) return;
    let disposed = false;
    setLoading(true);
    setFailure("");
    void enqueue(async () => (disposed ? null : loadDiff(file.path)))
      .then((result) => {
        if (!disposed && result) setData({ loader: loadDiff, content: result });
      })
      .catch((cause) => {
        if (!disposed) setFailure(message(cause));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
    // A cached result is retained through layout toggles and hide/show.
  }, [active, visible, collapsed, loadDiff, file.path, enqueue, retry]);
  useStatusIssues([
    {
      id: `review-file:${issueId}`,
      title: `${file.path} 的差异未能读取`,
      error: active ? failure : "",
      pending: active && loading,
      action: { label: "重试", run: () => setRetry((v) => v + 1) },
    },
  ]);
  return (
    <section
      ref={(element) => {
        root.current = element;
        register(file.path, element);
      }}
      className={`review-file-section${selected ? " selected" : ""}`}
      aria-label={`${file.path} 的变更`}
      onFocus={onSelect}
    >
      <header className="git-review-file-heading">
        <button
          className="review-file-toggle"
          aria-expanded={!collapsed}
          aria-label={`${collapsed ? "展开" : "折叠"} ${file.path}`}
          onClick={() => collapse((v) => !v)}
        >
          {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        </button>
        <FileCode2 size={16} />
        <strong title={file.path} aria-label={file.path}>
          <span className="review-directory-prefix">
            {file.path.slice(0, file.path.lastIndexOf("/") + 1)}
          </span>
          {file.path.split("/").at(-1)}
        </strong>
        <DiffStats file={file} />
        <span
          className={`review-file-status status-${file.status}`}
          title={statusLabels[file.status] ?? file.status}
        >
          {statusLabels[file.status] ?? file.status}
        </span>
      </header>
      {!collapsed &&
        (near ? (
          <div className="review-file-patch">
            {file.previous_path && (
              <p className="panel-notice">
                {file.previous_path} → {file.path}
              </p>
            )}
            {content ? (
              content.text ? (
                <ReviewDiff
                  content={content.text}
                  split={split}
                  wrap={wrap}
                  path={file.path}
                />
              ) : (
                <p className="panel-empty">
                  {file.binary
                    ? "二进制内容变化"
                    : "文件内容没有差异，可能仅权限发生变化，或工作区已更新。"}
                </p>
              )
            ) : (
              <p
                className="review-file-placeholder"
                role={loading ? "status" : undefined}
              >
                {failure
                  ? "差异暂不可用，请在状态提示中重试。"
                  : loading
                    ? "正在读取差异…"
                    : "滚动到此处读取差异"}
              </p>
            )}
            {content?.truncated && (
              <p className="panel-notice">此文件差异超出服务端读取范围。</p>
            )}
          </div>
        ) : (
          <div
            className="review-file-reserved"
            style={{ height: reservedHeight.current }}
            aria-hidden="true"
          />
        ))}
    </section>
  );
}

export function ReviewWorkspace({
  scopeControl,
  comparisonControl,
  files,
  added,
  removed,
  loading,
  unavailable,
  emptyLabel = "此范围没有变更",
  active,
  initialPath,
  boundary,
  notices,
  additionalActions,
  overlay,
  refresh,
  loadDiff,
}: {
  scopeControl: ReactNode;
  comparisonControl?: ReactNode;
  files: GitReviewFile[];
  added: number;
  removed: number;
  loading: boolean;
  unavailable?: string;
  emptyLabel?: string;
  active: boolean;
  initialPath?: string;
  boundary: string;
  notices?: ReactNode;
  additionalActions?: ReactNode;
  overlay?: ReactNode;
  refresh: () => void;
  loadDiff: Loader;
}) {
  const [selected, select] = useState(initialPath ?? files[0]?.path ?? "");
  const [split, setSplit] = useState(false),
    [wrap, setWrap] = useState(true),
    [filesVisible, showFiles] = useState(true);
  const [filter, setFilter] = useState("");
  const [closed, setClosed] = useState(new Set<string>());
  const sections = useRef(new Map<string, HTMLElement>());
  const [navigation, navigate] = useState({ path: "", revision: 0 });
  const initialJump = useRef("");
  const navigationLock = useRef("");
  const selectedFile = files.some((file) => file.path === selected)
    ? selected
    : initialPath && files.some((file) => file.path === initialPath)
      ? initialPath
      : (files[0]?.path ?? "");
  const enqueue = useMemo(diffQueue, []);
  const filtered = useMemo(
    () =>
      files.filter((file) =>
        `${file.path} ${file.previous_path ?? ""}`
          .toLowerCase()
          .includes(filter.trim().toLowerCase()),
      ),
    [files, filter],
  );
  const tree = useMemo(() => fileTree(filtered), [filtered]);
  function jump(path: string) {
    navigationLock.current = path;
    select(path);
    navigate((old) => ({ path, revision: old.revision + 1 }));
    sections.current
      .get(path)
      ?.scrollIntoView({ block: "start", behavior: "auto" });
  }
  useEffect(() => {
    if (
      initialPath &&
      initialJump.current !== initialPath &&
      sections.current.has(initialPath)
    ) {
      initialJump.current = initialPath;
      jump(initialPath);
    }
  }, [initialPath, files]);
  function toggle(path: string) {
    setClosed((old) => {
      const next = new Set(old);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }
  function renderTree(nodes: TreeNode[], depth = 0): ReactNode {
    return (
      <ul className="review-file-tree">
        {nodes.map((node) => (
          <li key={node.path}>
            {node.file ? (
              <button
                aria-pressed={node.path === selectedFile}
                className={node.path === selectedFile ? "selected" : ""}
                style={{ paddingLeft: 8 + depth * 12 }}
                title={node.path}
                onClick={() => jump(node.path)}
              >
                <FileCode2 size={15} />
                <span className="git-file-path">{node.name}</span>
                <DiffStats file={node.file} />
              </button>
            ) : (
              <>
                <button
                  className="review-tree-folder"
                  style={{ paddingLeft: 8 + depth * 12 }}
                  aria-expanded={!closed.has(node.path)}
                  title={node.path}
                  onClick={() => toggle(node.path)}
                  onKeyDown={(event) => {
                    if (["ArrowLeft", "ArrowRight"].includes(event.key)) {
                      event.preventDefault();
                      setClosed((old) => {
                        const next = new Set(old);
                        if (event.key === "ArrowLeft") next.add(node.path);
                        else next.delete(node.path);
                        return next;
                      });
                    }
                  }}
                >
                  {closed.has(node.path) ? (
                    <ChevronRight size={13} />
                  ) : (
                    <ChevronDown size={13} />
                  )}
                  <Folder size={14} />
                  <span className="git-file-path">{node.name}</span>
                </button>
                {!closed.has(node.path) && renderTree(node.children, depth + 1)}
              </>
            )}
          </li>
        ))}
      </ul>
    );
  }
  return (
    <div className="git-review">
      <div className="git-review-toolbar">
        <div className="review-scope-control">
          <div className="review-scope-summary">
            {scopeControl}
            {!loading && files.length > 0 && (
              <span className="change-totals">
                <b>+{added}</b>
                <em>-{removed}</em>
              </span>
            )}
          </div>
          {comparisonControl}
        </div>
        <div className="git-review-actions">
          {additionalActions}
          <button
            className="icon-button"
            aria-label="刷新变更"
            title="刷新变更"
            disabled={!active || loading}
            onClick={refresh}
          >
            <RefreshCw size={16} />
          </button>
          <button
            className="icon-button"
            aria-label="自动换行"
            title="自动换行"
            aria-pressed={wrap}
            onClick={() => setWrap((v) => !v)}
          >
            <WrapText size={16} />
          </button>
          <button
            className="icon-button"
            aria-label="并排差异"
            title="并排差异"
            aria-pressed={split}
            onClick={() => setSplit((v) => !v)}
          >
            <Columns2 size={16} />
          </button>
          <button
            className="icon-button"
            aria-label="文件列表"
            title="文件列表"
            aria-pressed={filesVisible}
            onClick={() => showFiles((v) => !v)}
          >
            <PanelRight size={16} />
          </button>
        </div>
      </div>
      {overlay}
      {notices}
      <div className={`git-review-body${filesVisible ? "" : " files-hidden"}`}>
        <div
          className="git-review-content"
          aria-label="文件差异浏览"
          onWheel={(event) => {
            if (event.deltaY) navigationLock.current = "";
          }}
          onPointerDown={() => {
            navigationLock.current = "";
          }}
          onTouchStart={() => {
            navigationLock.current = "";
          }}
          onKeyDown={(event) => {
            if (
              [
                "ArrowUp",
                "ArrowDown",
                "PageUp",
                "PageDown",
                "Home",
                "End",
              ].includes(event.key)
            )
              navigationLock.current = "";
          }}
          onScroll={(event) => {
            if (navigationLock.current) return;
            const top = event.currentTarget.scrollTop + 1;
            let current = "";
            for (const file of files) {
              const section = sections.current.get(file.path);
              if (!section || section.offsetTop > top) break;
              current = file.path;
            }
            if (current) select(current);
          }}
        >
          {loading ? (
            <p className="panel-empty" role="status">
              正在读取变更…
            </p>
          ) : unavailable ? (
            <p className="panel-empty">{unavailable}</p>
          ) : !files.length ? (
            <p className="panel-empty">{emptyLabel}</p>
          ) : (
            files.map((file, index) => (
              <ReviewFileSection
                key={file.path}
                file={file}
                active={active}
                initiallyVisible={file.path === selectedFile || index === 0}
                selected={file.path === selectedFile}
                loadDiff={loadDiff}
                enqueue={enqueue}
                split={split}
                wrap={wrap}
                register={(path, element) => {
                  if (element) sections.current.set(path, element);
                  else sections.current.delete(path);
                }}
                reveal={navigation.path === file.path ? navigation.revision : 0}
                onSelect={() => select(file.path)}
              />
            ))
          )}
        </div>
        {filesVisible && (
          <aside className="git-review-files" aria-label="变更文件">
            <div className="git-review-filter">
              <Search size={14} />
              <input
                aria-label="筛选文件"
                placeholder="筛选文件…"
                spellCheck={false}
                value={filter}
                onChange={(event) => {
                  setFilter(event.target.value);
                  setClosed(new Set());
                }}
              />
            </div>
            <div className="git-review-file-count">
              {filtered.length} / {files.length} 个文件
            </div>
            <nav className="git-review-file-list" aria-label="变更目录">
              {renderTree(tree)}
              {!loading && !filtered.length && (
                <p className="panel-notice">
                  {files.length ? "没有匹配的文件" : "没有变更文件"}
                </p>
              )}
            </nav>
          </aside>
        )}
      </div>
      <p className="git-review-boundary" title={boundary}>
        {boundary}
      </p>
    </div>
  );
}
