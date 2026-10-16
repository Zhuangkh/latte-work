import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, FileText, Folder } from "lucide-react";
import { message, request } from "./api";
import type { FileEntry } from "./protocol";
import { useStatusIssues } from "./statusNotices";

interface TreeProps {
  hostId: string;
  projectId: string;
  reference?: FileEntry;
  sessionId?: string;
  file: string;
  directoryPath?: string;
  active: boolean;
  revision: number;
  openFile: (path: string) => void;
}

export function WorkspaceFileTree(props: TreeProps) {
  if (props.reference && !props.reference.directory)
    return (
      <ul className="file-tree-list">
        <Entry {...props} entry={props.reference} />
      </ul>
    );
  return <Directory {...props} path={props.reference?.path ?? ""} expanded />;
}

function Directory({
  path,
  expanded,
  ...props
}: TreeProps & { path: string; expanded: boolean }) {
  const { hostId, projectId, active, revision, reference, sessionId } = props;
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!active || !expanded) return;
    let disposed = false;
    setLoading(true);
    setError("");
    void request(
      hostId,
      reference
        ? {
            method: "preview_source",
            project_id: projectId,
            session_id: sessionId ?? null,
            path,
            offset: 0,
          }
        : { method: "files", project_id: projectId, path },
    )
      .then((response) => {
        if (response.kind !== "files" && response.kind !== "source_directory")
          throw new Error("无法读取项目文件，请检查 Server 版本");
        if (!disposed) setEntries(response.entries);
      })
      .catch((cause) => {
        if (!disposed) setError(message(cause));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [
    hostId,
    projectId,
    path,
    active,
    expanded,
    revision,
    retry,
    reference,
    sessionId,
  ]);
  useStatusIssues([
    {
      id: `file-tree:${hostId}:${projectId}:${path}`,
      title: `${path || "项目目录"} 未能读取`,
      error: active && expanded ? error : "",
      pending: active && expanded && loading,
      action: { label: "重试", run: () => setRetry((value) => value + 1) },
    },
  ]);
  if (!expanded) return null;
  return (
    <ul className="file-tree-list">
      {loading && (
        <li className="file-tree-notice" role="status">
          正在读取…
        </li>
      )}
      {entries.map((entry) => (
        <Entry key={entry.path} {...props} entry={entry} />
      ))}
      {!loading && !error && entries.length === 0 && (
        <li className="file-tree-notice">目录为空</li>
      )}
    </ul>
  );
}

function Entry({ entry, ...props }: TreeProps & { entry: FileEntry }) {
  const containsFile =
    props.file.startsWith(`${entry.path}/`) ||
    props.directoryPath === entry.path ||
    !!props.directoryPath?.startsWith(`${entry.path}/`);
  const [expanded, setExpanded] = useState(containsFile);
  useEffect(() => {
    if (containsFile) setExpanded(true);
  }, [props.file, props.directoryPath, containsFile]);
  return (
    <li>
      <button
        className="file-tree-entry"
        title={entry.path}
        aria-expanded={entry.directory ? expanded : undefined}
        aria-current={
          !entry.directory && entry.path === props.file ? "page" : undefined
        }
        onClick={() =>
          entry.directory
            ? setExpanded((value) => !value)
            : props.openFile(entry.path)
        }
      >
        {entry.directory ? (
          <>
            {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            <Folder size={15} />
          </>
        ) : (
          <FileText size={15} />
        )}
        <span>{entry.name}</span>
      </button>
      {entry.directory && (
        <Directory {...props} path={entry.path} expanded={expanded} />
      )}
    </li>
  );
}
