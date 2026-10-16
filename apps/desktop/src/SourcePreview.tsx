import { useEffect, useRef, useState } from "react";
import { FileText, Image, Plug, Wrench } from "lucide-react";
import { message } from "./api";
import { CodeView } from "./CodeView";
import {
  loadSource,
  systemPreview,
  previewPdf,
  type SourceData,
} from "./sourceLoader";
import { useStatusIssues } from "./statusNotices";
import type { SourceItem } from "./useTaskSources";
function usePreview(
  hostId: string,
  projectId: string,
  sessionId: string | undefined,
  source: SourceItem,
  active: boolean,
  revision = 0,
) {
  const scope = `${hostId}:${projectId}:${sessionId}:${source.path}:${revision}`;
  const [state, setState] = useState<{
    scope: string;
    data?: SourceData;
    error?: string;
    url?: string;
  }>({ scope });
  useEffect(() => {
    if (!active || source.sourceKind) return;
    let disposed = false,
      url: string | undefined;
    setState({ scope });
    void loadSource(hostId, projectId, sessionId, source.path, () => !disposed)
      .then(async (data) => {
        if (disposed) return;
        let bytes = data.bytes;
        if (data.mime === "application/pdf") {
          try {
            bytes = new Uint8Array(await previewPdf(source.name, data.bytes));
          } catch (error) {
            if (!disposed) setState({ scope, data, error: message(error) });
            return;
          }
        }
        if (disposed) return;
        if (/^(image\/|video\/|application\/pdf$)/.test(data.mime))
          url = URL.createObjectURL(
            new Blob([bytes as BlobPart], {
              type: data.mime === "application/pdf" ? "image/png" : data.mime,
            }),
          );
        setState({ scope, data, url });
      })
      .catch((error) => {
        if (!disposed) setState({ scope, error: message(error) });
      });
    return () => {
      disposed = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [
    scope,
    active,
    source.sourceKind,
    hostId,
    projectId,
    sessionId,
    source.path,
  ]);
  return state.scope === scope ? state : { scope };
}
export function SourceThumbnail({
  source,
  hostId,
  projectId,
  sessionId,
}: {
  source: SourceItem;
  hostId: string;
  projectId: string;
  sessionId?: string;
}) {
  const isImage =
    !source.sourceKind &&
    (source.attachmentKind === "image" ||
      /\.(png|jpe?g|gif|webp)$/i.test(source.name));
  const target = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!isImage || !target.current) return;
    if (!window.IntersectionObserver) {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "80px" },
    );
    observer.observe(target.current);
    return () => observer.disconnect();
  }, [isImage, source.path]);
  const preview = usePreview(
    hostId,
    projectId,
    sessionId,
    source,
    isImage && visible,
  );
  if (source.sourceKind === "connector") return <Plug size={20} />;
  if (source.sourceKind === "tool") return <Wrench size={20} />;
  return (
    <span ref={target} className="source-thumbnail-frame">
      {isImage && preview.url && preview.data?.mime.startsWith("image/") ? (
        <img className="source-thumbnail" src={preview.url} alt="" />
      ) : isImage ? (
        <Image size={20} />
      ) : (
        <FileText size={20} />
      )}
    </span>
  );
}
export function SourcePreview({
  hostId,
  projectId,
  sessionId,
  source,
  active,
  open,
  embedded = false,
  wrap = false,
}: {
  hostId: string;
  projectId: string;
  sessionId?: string;
  source: SourceItem;
  active: boolean;
  open: (source: SourceItem) => void;
  embedded?: boolean;
  wrap?: boolean;
}) {
  const [revision, refresh] = useState(0);
  const preview = usePreview(
    hostId,
    projectId,
    sessionId,
    source,
    active,
    revision,
  );
  const [systemError, setSystemError] = useState("");
  const [busy, setBusy] = useState(false);
  const current = useRef("");
  current.current = `${hostId}:${projectId}:${sessionId}:${source.path}:${active}:${revision}`;
  const scope = current.current;
  useEffect(() => {
    setBusy(false);
    setSystemError("");
  }, [scope]);
  const data = preview.data;
  useStatusIssues(
    embedded
      ? [
          {
            id: `reference-file:${hostId}:${projectId}:${source.path}`,
            title: `${source.name} 未能读取`,
            error: active ? (preview.error ?? "") : "",
            pending: active && !data && !preview.error,
            action: { label: "重试", run: () => refresh((v) => v + 1) },
          },
        ]
      : [],
  );
  async function nativePreview() {
    setBusy(true);
    setSystemError("");
    try {
      const loaded =
        data && !data.truncated
          ? data
          : await loadSource(
              hostId,
              projectId,
              sessionId,
              source.path,
              () => current.current === scope,
              true,
            );
      if (current.current === scope)
        await systemPreview(source.name, loaded.bytes);
    } catch (error) {
      if (current.current === scope) setSystemError(message(error));
    } finally {
      if (current.current === scope) setBusy(false);
    }
  }
  if (source.sourceKind)
    return (
      <div className="source-preview">
        <h2>{source.name}</h2>
        <p>
          {source.sourceKind === "connector" ? "连接器" : "工具"} · 已调用{" "}
          {source.uses ?? 0} 次
        </p>
        <p className="muted">当前任务实际调用的工具</p>
        <ul>
          {source.tools?.map((tool) => (
            <li key={tool}>{tool}</li>
          ))}
        </ul>
      </div>
    );
  return (
    <div
      className={embedded ? "workspace-reference-content" : "source-preview"}
    >
      {!embedded && (
        <>
          <div className="workspace-file-heading">
            <span>{source.name}</span>
            <button onClick={() => refresh((v) => v + 1)}>刷新</button>
          </div>
          <p className="source-path">{source.path}</p>
        </>
      )}
      {!embedded && preview.error && (
        <p role="alert" className="notice failure">
          {preview.error}
        </p>
      )}
      {!data && !preview.error && <p className="muted">正在读取…</p>}
      {data?.directory ? (
        <div>
          {data.directory.map((entry) => (
            <button
              key={entry.path}
              className="task-source-row"
              onClick={() => open(entry)}
            >
              <FileText size={18} />
              <span>
                {entry.name}
                {entry.directory ? "/" : ""}
              </span>
            </button>
          ))}
        </div>
      ) : (
        data && (
          <>
            {(!embedded || data.mime !== "text/plain") && (
              <p className="muted">
                {data.mime} · {(data.size / 1024).toFixed(1)} KiB
              </p>
            )}
            {data.mime.startsWith("image/") && preview.url ? (
              <img
                className="source-image-preview"
                src={preview.url}
                alt={source.name}
              />
            ) : data.mime.startsWith("video/") && preview.url ? (
              <video
                className="source-video-preview"
                src={preview.url}
                controls
                preload="metadata"
              />
            ) : data.mime === "application/pdf" && preview.url ? (
              <>
                <p className="muted">PDF 首页 · 系统预览可查看完整文件</p>
                <img
                  className="source-image-preview"
                  src={preview.url}
                  alt={`${source.name} 首页`}
                />
              </>
            ) : data.mime === "text/plain" ? (
              <CodeView
                content={new TextDecoder().decode(data.bytes)}
                wrap={wrap}
              />
            ) : (
              <p>此格式可使用系统预览打开。</p>
            )}
            {(!embedded || data.mime !== "text/plain") && (
              <button
                disabled={busy || !active}
                onClick={() => void nativePreview()}
              >
                {busy ? "正在准备预览…" : "系统预览"}
              </button>
            )}
          </>
        )
      )}
      {data?.truncated && <p className="muted">内容已截断</p>}
      {systemError && (
        <p className="notice failure" role="alert">
          {systemError}
        </p>
      )}
    </div>
  );
}
