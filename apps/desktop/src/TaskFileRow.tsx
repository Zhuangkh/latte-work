import { useEffect, useRef, useState } from "react";
import { FileText, Folder } from "lucide-react";
import { request, message } from "./api";
import type { Project } from "./protocol";
import type { InfoFile } from "./taskInfoModel";
import { openArtifact } from "./workspaceState";
import { useStatusIssues } from "./statusNotices";

export function TaskFileRow({
  file,
  workspaceId,
  hostId,
  project,
  sessionId,
  active,
  connected,
}: {
  file: InfoFile;
  workspaceId: string;
  hostId: string;
  project?: Project;
  sessionId?: string;
  active: boolean;
  connected: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const scope = JSON.stringify([
    workspaceId,
    hostId,
    project?.id,
    sessionId,
    active,
    connected,
    file.path,
  ]);
  const current = useRef({ scope, version: 0, mounted: true });
  if (current.current.scope !== scope)
    current.current = {
      ...current.current,
      scope,
      version: current.current.version + 1,
    };
  const version = current.current.version;
  const valid = () =>
    current.current.mounted && current.current.version === version;
  useEffect(() => {
    current.current.mounted = true;
    return () => {
      current.current.mounted = false;
    };
  }, []);
  useEffect(() => {
    setBusy(false);
    setError("");
  }, [scope]);
  async function open() {
    if (busy || !project || !active || !connected) return;
    setBusy(true);
    try {
      const path = file.path.startsWith("/")
        ? file.path
        : `${project.path.replace(/\/$/, "")}/${file.path}`;
      const response = await request(hostId, {
        method: "resolve_reference",
        project_id: project.id,
        path,
      });
      if (!valid()) return;
      if (response.kind !== "file_reference")
        throw new Error("此 Server 不支持文件打开，请更新 Server");
      openArtifact(workspaceId, response.entry, { hostId, project, sessionId });
      setError("");
    } catch (cause) {
      if (valid()) setError(message(cause));
    } finally {
      if (valid()) setBusy(false);
    }
  }
  useStatusIssues([
    {
      id: `task-file:${hostId}:${project?.id}:${file.path}`,
      title: `${file.name} 未能打开`,
      error: active ? error : "",
      pending: busy,
      action: { label: "重试", run: () => void open() },
    },
  ]);
  const Icon = file.directory ? Folder : FileText;
  return (
    <button
      className="task-info-file"
      title={`${file.directory ? "打开文件夹" : "打开文件"}\n${file.path}`}
      disabled={!connected || !project || busy}
      onClick={() => void open()}
    >
      <Icon size={16} />
      <span>
        <span className="task-info-resource-title" title={file.name}>
          {file.name}
        </span>
        <small>{file.path}</small>
      </span>
    </button>
  );
}
