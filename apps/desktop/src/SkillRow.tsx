import { useEffect, useRef, useState } from "react";
import { HoverCard } from "./SidebarHoverCard";
import { message, request } from "./api";
import type { Project, SkillInfo } from "./protocol";
import { openArtifact } from "./workspaceState";
import { useStatusIssues } from "./statusNotices";

export function SkillRow({
  name,
  uses,
  workspaceId,
  hostId,
  sessionId,
  project,
  connected,
  active,
}: {
  name: string;
  uses: number;
  workspaceId: string;
  hostId: string;
  sessionId?: string;
  project?: Project;
  connected: boolean;
  active: boolean;
}) {
  const [info, setInfo] = useState<SkillInfo | null>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const pending = useRef<Promise<SkillInfo | null> | null>(null);
  const context = JSON.stringify([hostId, sessionId, name, active, connected]);
  const current = useRef({ context, version: 0, mounted: true });
  if (current.current.context !== context)
    current.current = {
      ...current.current,
      context,
      version: current.current.version + 1,
    };
  useEffect(() => {
    current.current.mounted = true;
    return () => {
      current.current.mounted = false;
    };
  }, []);
  const token = () => current.current.version;
  const valid = (version: number) =>
    current.current.mounted && current.current.version === version;
  async function load(force = false) {
    if (info !== undefined && !force) return info;
    if (!connected || !sessionId || !active) return null;
    if (pending.current) return pending.current;
    const version = token();
    const work = request(hostId, {
      method: "skill_info",
      session_id: sessionId,
      name,
    })
      .then((response) => {
        if (response.kind !== "skill_info")
          throw new Error("此 Server 不支持 Skill 信息，请更新 Server");
        if (valid(version)) {
          setInfo(response.skill);
          setError("");
        }
        return response.skill;
      })
      .finally(() => {
        if (pending.current === work) pending.current = null;
      });
    pending.current = work;
    return work;
  }
  function inspect(force = false) {
    const version = token();
    void load(force).catch((cause) => {
      if (valid(version)) setError(message(cause));
    });
  }
  async function open() {
    if (
      busy ||
      info === null ||
      !active ||
      !connected ||
      !project ||
      !sessionId
    )
      return;
    const version = token();
    setBusy(true);
    try {
      const skill = await load();
      if (!valid(version)) return;
      if (!skill) throw new Error("未找到此 Skill 的可读取目录");
      // Opening is an explicit file selection; metadata hover never grants preview access.
      const response = await request(hostId, {
        method: "resolve_reference",
        project_id: project.id,
        path: skill.directory,
      });
      if (!valid(version)) return;
      if (response.kind !== "file_reference" || !response.entry.directory)
        throw new Error("Skill 目录暂不可用");
      setError("");
      openArtifact(
        workspaceId,
        { ...response.entry, name: skill.name },
        { hostId, project, sessionId },
        `${response.entry.path.replace(/\/$/, "")}/SKILL.md`,
      );
    } catch (cause) {
      if (valid(version)) setError(message(cause));
    } finally {
      if (current.current.mounted) setBusy(false);
    }
  }
  useStatusIssues([
    {
      id: `skill:${hostId}:${sessionId}:${name}`,
      title: `${name} 暂不可用`,
      error: active ? error : "",
      pending: busy,
      action: { label: "重试", run: () => inspect(true) },
    },
  ]);
  return (
    <HoverCard
      content={
        <>
          <div className="sidebar-hover-title">{info?.name ?? name}</div>
          {info?.name && info.name !== name && (
            <div className="skill-hover-path">{name}</div>
          )}
          <p className="skill-hover-description">
            {info?.description ||
              (error
                ? "信息暂不可用，可在状态提示中重试"
                : info === null
                  ? "未找到可读取的 Skill 文件"
                  : !connected
                    ? "主机未连接"
                    : info === undefined
                      ? "正在读取…"
                      : "此 Skill 未提供描述")}
          </p>
          {info && (
            <div className="skill-hover-path">
              {info.directory}
              <br />
              打开 SKILL.md
            </div>
          )}
        </>
      }
    >
      <button
        className="task-info-row task-info-skill"
        aria-label={`查看 Skill ${name}`}
        title="打开 SKILL.md"
        aria-disabled={!connected || busy || info === null}
        onMouseEnter={() => inspect()}
        onFocus={() => inspect()}
        onClick={() => void open()}
      >
        <span>{name}</span>
        <small>{uses} 次</small>
      </button>
    </HoverCard>
  );
}
