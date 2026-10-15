import { useCallback, useEffect, useId, useRef, useState } from "react";
import { ArrowRight, MoreHorizontal } from "lucide-react";
import type {
  ChangeSummary,
  GitCommit,
  GitInfo,
  GitReview,
  Project,
  TurnChanges,
} from "./protocol";
import { request, message } from "./api";
import { ChangeScopePicker, type ReviewScope } from "./ChangeScopePicker";
import { ReviewOptions } from "./ReviewOptions";
import { ReviewWorkspace } from "./ReviewWorkspace";
import { formatMessageTimestamp } from "./MessageTimestamp";
import { Select } from "./Select";
import { useStatusIssues } from "./statusNotices";
type Snapshot = {
  scope: ReviewScope;
  context: string;
  review: GitReview;
  turn?: TurnChanges;
  summary?: ChangeSummary;
  sessionId?: string;
};
export function ChangesView({
  hostId,
  project,
  active,
  sessionId,
  lastTurnVersion,
  requestId,
  snapshotAt,
  initialPath,
  initialScope = requestId ? "turn" : "branch",
}: {
  hostId: string;
  project: Project;
  active: boolean;
  sessionId?: string;
  lastTurnVersion?: string;
  requestId?: string;
  snapshotAt?: number | null;
  initialPath?: string;
  initialScope?: ReviewScope;
}) {
  const [info, setInfo] = useState<GitInfo>();
  const [scope, setScope] = useState<ReviewScope>(initialScope);
  const [base, setBase] = useState("");
  const [commit, setCommit] = useState<GitCommit>();
  const gitMode = [
    "branch",
    "commit",
    "worktree",
    "unstaged",
    "staged",
  ].includes(scope);
  const context = JSON.stringify([
    hostId,
    project.id,
    scope,
    gitMode ? null : sessionId,
    scope === "turn" ? requestId : null,
    scope === "branch" ? base : scope === "commit" ? commit?.oid : null,
  ]);
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const review = snapshot?.context === context ? snapshot.review : undefined;
  const turn = review ? snapshot?.turn : undefined;
  const summary = review ? snapshot?.summary : undefined;
  const [selected, select] = useState(initialPath ?? "");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [revision, refresh] = useState(0);
  const issueId = useId();
  useStatusIssues([
    {
      id: `changes:${issueId}`,
      title: `${project.name} 的变更未能读取`,
      error: active ? error : "",
      pending: active && loading,
      action: { label: "重试", run: () => refresh((value) => value + 1) },
    },
  ]);
  const [fullContext, setFullContext] = useState(false);
  const [more, setMore] = useState(false);
  const moreTrigger = useRef<HTMLButtonElement>(null);
  const closeMore = useCallback((restoreFocus = true) => {
    setMore(false);
    if (restoreFocus) moreTrigger.current?.focus();
  }, []);
  const turnRevision = gitMode ? 0 : revision;
  const infoContext = useRef("");
  const selection = useRef(selected);
  selection.current = selected;
  useEffect(() => {
    select(initialPath ?? "");
  }, [initialPath]);
  useEffect(() => {
    infoContext.current = "";
    setInfo(undefined);
    setBase("");
    setSnapshot(undefined);
    select(initialPath ?? "");
    setError("");
  }, [hostId, project.id]);
  useEffect(() => {
    if (!active || !gitMode) return;
    setLoading(true);
    setError("");
    let disposed = false;
    void request(hostId, { method: "git_info", project_id: project.id })
      .then((response) => {
        if (response.kind !== "git_info")
          throw new Error("Server 不支持分支审阅，请更新后重试");
        if (!disposed) {
          infoContext.current = JSON.stringify([hostId, project.id]);
          setInfo({ ...response.info });
          setBase(
            (previous) => previous || response.info.default_base || "HEAD",
          );
        }
      })
      .catch((cause) => {
        if (!disposed) {
          setError(message(cause));
          setLoading(false);
        }
      });
    return () => {
      disposed = true;
    };
  }, [active, hostId, project.id, revision, gitMode]);
  useEffect(() => {
    if (
      !active ||
      (gitMode &&
        (!info ||
          infoContext.current !== JSON.stringify([hostId, project.id]) ||
          (scope === "branch" && !base)))
    )
      return;
    let disposed = false;
    setLoading(true);
    setError("");
    setSnapshot(undefined);
    setMore(false);
    if (!gitMode && !sessionId) {
      setLoading(false);
      select("");
      return;
    }
    void request(
      hostId,
      scope === "lastTurn"
        ? {
            method: "last_turn_changes",
            session_id: sessionId!,
          }
        : scope === "turn"
          ? {
              method: "turn_change_summary",
              session_id: sessionId!,
              request_id: requestId!,
            }
          : scope === "task"
            ? {
                method: "change_summary",
                project_id: project.id,
                session_id: sessionId!,
              }
            : {
                method: "git_review",
                project_id: project.id,
                scope: scope as Exclude<
                  ReviewScope,
                  "lastTurn" | "turn" | "task"
                >,
                base:
                  scope === "branch"
                    ? base
                    : scope === "commit"
                      ? commit!.oid
                      : null,
              },
    )
      .then((response) => {
        if (disposed) return;
        if (
          scope === "lastTurn" &&
          response.kind === "last_turn_changes" &&
          !response.changes
        ) {
          select("");
          return;
        }
        const changes =
          response.kind === "last_turn_changes"
            ? (response.changes ?? undefined)
            : response.kind === "turn_changes"
              ? response.changes
              : undefined;
        const summary =
          changes?.summary ??
          (response.kind === "change_summary" ? response.summary : undefined);
        const result: GitReview | undefined = !gitMode
          ? summary && {
              base: null,
              head: null,
              entries: summary.entries.map((entry) => ({
                ...entry,
                previous_path: null,
                binary: false,
                untracked: false,
              })),
              added: summary.added,
              removed: summary.removed,
              truncated: summary.truncated,
            }
          : response.kind === "git_review"
            ? response.review
            : undefined;
        if (!result)
          throw new Error(
            !gitMode
              ? "Server 不支持回合快照，请更新后重试"
              : "Server 不支持 Git 审阅，请更新后重试",
          );
        if (!disposed) {
          setSnapshot({
            scope,
            context,
            review: result,
            turn: changes,
            summary,
            sessionId,
          });
          setError(summary?.unavailable ?? "");
          select(
            result.entries.some((file) => file.path === selection.current)
              ? selection.current
              : (result.entries[0]?.path ?? ""),
          );
        }
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
    active,
    hostId,
    project.id,
    scope,
    base,
    info,
    sessionId,
    scope === "lastTurn" ? lastTurnVersion : null,
    turnRevision,
    requestId,
    commit,
    context,
    gitMode,
  ]);
  const loadDiff = useCallback(
    async (path: string) => {
      const response = await request(
        hostId,
        scope === "lastTurn" || scope === "turn"
          ? {
              method: "turn_change_diff",
              session_id: snapshot!.sessionId!,
              request_id: turn!.request_id,
              path,
            }
          : scope === "task"
            ? {
                method: "task_change_diff",
                session_id: snapshot!.sessionId!,
                path,
              }
            : {
                method: "git_review_diff",
                project_id: project.id,
                scope: scope as Exclude<
                  ReviewScope,
                  "lastTurn" | "turn" | "task"
                >,
                base: review!.base,
                head: review!.head,
                path,
                full_context: fullContext,
              },
      );
      if (response.kind !== "content") throw new Error("无法读取文件差异");
      return response;
    },
    [hostId, project.id, scope, snapshot, turn, review, fullContext],
  );
  const options = info
    ? [
        ...info.refs.map((ref) => ({ value: ref.full_name, label: ref.name })),
        { value: "HEAD", label: "当前提交" },
      ]
    : [];
  const scopeControl = (
    <>
      <ChangeScopePicker
        hostId={hostId}
        projectId={project.id}
        sessionId={sessionId}
        requestId={requestId}
        scope={scope}
        commit={commit}
        disabled={!active}
        choose={(value, commit) => {
          setScope(value);
          if (commit) setCommit(commit);
          select(value === "turn" ? (initialPath ?? "") : "");
        }}
      />
    </>
  );
  return (
    <ReviewWorkspace
      key={JSON.stringify([hostId, project.id, sessionId, requestId])}
      scopeControl={scopeControl}
      comparisonControl={
        scope === "branch" ? (
          <div className="git-review-comparison">
            <span title={info?.head ?? ""}>
              {info?.branch ?? (info?.head ? "分离 HEAD" : "分支")}
            </span>
            <ArrowRight size={14} />
            <Select
              label="比较基准"
              compact
              value={base}
              options={options}
              onChange={setBase}
              disabled={!active || !info}
              minMenuWidth={200}
              searchable
              searchPlaceholder="搜索分支"
            />
          </div>
        ) : null
      }
      files={review?.entries ?? []}
      added={review?.added ?? 0}
      removed={review?.removed ?? 0}
      active={active}
      initialPath={selected}
      loading={loading || (gitMode && !info && !error)}
      unavailable={error || undefined}
      emptyLabel={
        scope === "lastTurn" && !turn
          ? "当前对话尚无已保存的回合快照"
          : !gitMode
            ? summary?.truncated
              ? "采集范围内没有已记录的文件变更"
              : `${scope === "task" ? "本任务" : "本轮"}没有文件变更`
            : "此范围没有变更"
      }
      loadDiff={loadDiff}
      refresh={() => refresh((v) => v + 1)}
      additionalActions={
        <button
          ref={moreTrigger}
          className="icon-button"
          title="更多差异选项"
          aria-label="更多差异选项"
          aria-haspopup="menu"
          aria-expanded={more}
          disabled={!review?.entries.length || !active}
          onClick={() => setMore((v) => !v)}
        >
          <MoreHorizontal size={16} />
        </button>
      }
      overlay={
        more && active && moreTrigger.current ? (
          <ReviewOptions
            trigger={moreTrigger.current}
            fullContext={fullContext}
            frozen={!gitMode}
            choose={() => setFullContext((v) => !v)}
            close={closeMore}
          />
        ) : null
      }
      notices={
        <>
          {turn &&
            (turn.undo === "reverted" ||
              turn.undo === "unknown" ||
              turn.undo_unavailable ||
              turn.interrupted ||
              turn.background_pending) && (
              <p className="panel-notice">
                {[
                  turn.undo === "reverted"
                    ? "本轮修改已撤销，仍显示原始快照。"
                    : turn.undo === "unknown"
                      ? "撤销状态待核对，仍显示原始快照。"
                      : "",
                  turn.undo_unavailable,
                  turn.interrupted ? "本轮已中断，保留已记录的修改。" : "",
                  turn.background_pending
                    ? "采集快照时后台任务尚未结束，保留当时已记录的修改。"
                    : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
              </p>
            )}
          {review?.truncated && (
            <p className="panel-notice">部分文件或统计超出采集范围。</p>
          )}
          {gitMode && info?.truncated && (
            <p className="panel-notice">分支列表仅显示部分结果。</p>
          )}
        </>
      }
      boundary={
        scope === "task"
          ? "任务开始后的净变更；共享目录内可能包含其他写入。"
          : scope === "turn"
            ? `${snapshotAt ? (formatMessageTimestamp(snapshotAt) ?? "") + " · " : ""}对比本轮开始与结束时保存的内容；后续修改不会改变快照。共享目录中的同期写入也可能包含在内。`
            : scope === "commit"
              ? "所选提交相对第一父级的变更；首次提交对比空目录，不包含未提交修改。"
              : scope === "lastTurn"
                ? "对比上一轮开始与结束时保存的内容；后续修改不会改变快照。共享目录中的同期写入也可能包含在内。"
                : scope === "branch"
                  ? "对比当前提交与基准分支的共同祖先；未提交的修改在未提交范围查看。"
                  : scope === "worktree"
                    ? "当前工作区相对 HEAD 的净变更，包含暂存及未跟踪文件。"
                    : scope === "staged"
                      ? "暂存区相对 HEAD 的变更。"
                      : "工作区相对暂存区的变更，包含未跟踪文件。"
      }
    />
  );
}
