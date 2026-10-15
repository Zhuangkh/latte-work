import {
  ChevronRight,
  ShieldCheck,
  Terminal,
  FileText,
  FilePenLine,
  Search,
  Wrench,
  MessageCircleQuestion,
} from "lucide-react";
import type { Event } from "./protocol";
import { toolLabel, type ToolActivity } from "./activity";
import { ApprovalOperation } from "./ApprovalOperation";

type Request = Extract<Event["event"], { kind: "approval" }>;
export function ApprovalCard({
  request,
  resolved,
  decision,
  tool,
  disabled,
  busy,
  presentation = "request",
  onDecision,
}: {
  request: Request;
  resolved?: boolean;
  decision?: "allowed" | "denied" | "expired";
  tool?: ToolActivity;
  disabled?: boolean;
  busy?: boolean;
  presentation?: "request" | "record";
  onDecision: (allow: boolean) => void;
}) {
  const label = toolLabel({
    key: 0,
    type: "tool",
    name: request.tool,
    input: request.input,
    status: "waiting",
  });
  const kind = request.tool.toLowerCase();
  if (kind === "askuserquestion") {
    if (resolved || presentation === "record")
      return (
        <details className="activity-row question-result">
          <summary>
            <MessageCircleQuestion size={16} />
            <span className="activity-label">提问（旧记录）</span>
            <span className="activity-status">
              {resolved ? "未保存结构化回答" : "需要更新服务"}
            </span>
            <ChevronRight size={14} className="activity-chevron" />
          </summary>
          <ApprovalOperation input={request.input} compact />
        </details>
      );
    return (
      <section
        className="approval-card question-card"
        aria-label="问题交互暂不可用"
      >
        <header>
          <MessageCircleQuestion size={16} />
          需要你的回答
        </header>
        <p>
          当前主机服务尚不支持问题交互，请更新服务后使用。你也可以跳过这次问题，让模型继续。
        </p>
        <div className="approval-actions">
          <button
            className="secondary"
            disabled={disabled || busy}
            onClick={() => onDecision(false)}
          >
            {busy ? "正在提交…" : "跳过问题"}
          </button>
        </div>
      </section>
    );
  }
  const [Icon, category] =
    kind === "bash"
      ? ([Terminal, "终端"] as const)
      : kind === "write"
        ? ([FileText, "写入文件"] as const)
        : ["edit", "multiedit"].includes(kind)
          ? ([FilePenLine, "编辑文件"] as const)
          : kind === "read"
            ? ([FileText, "读取文件"] as const)
            : ["glob", "grep"].includes(kind)
              ? ([Search, "搜索"] as const)
              : ([Wrench, request.tool] as const);
  const question =
    kind === "bash" ? "是否允许运行以下命令？" : `是否允许${label}？`;
  const status = !resolved
    ? "等待审批"
    : decision === "denied"
      ? "已拒绝"
      : decision !== "allowed"
        ? "审批已过期"
        : tool?.status === "failed"
          ? "执行失败"
          : tool?.status === "completed"
            ? "已完成"
            : tool?.status === "pending"
              ? "已允许 · 执行中"
              : tool?.status === "unconfirmed"
                ? "已允许 · 结果待确认"
                : "已允许";
  const parameters = (
    <details className="approval-parameters">
      <summary>
        操作参数
        <ChevronRight size={14} aria-hidden="true" />
      </summary>
      <pre>{JSON.stringify(request.input, null, 2)}</pre>
    </details>
  );
  if (resolved || presentation === "record")
    return (
      <details
        className={`approval-result activity-row ${tool?.status === "failed" ? "failure" : ""}`}
      >
        <summary>
          <ShieldCheck size={16} />
          <span className="activity-label">{label}</span>
          <span className="activity-status">{status}</span>
          <ChevronRight size={14} className="activity-chevron" />
        </summary>
        <div className="approval-result-body">
          <ApprovalOperation input={request.input} />
          {parameters}
          {tool?.output !== undefined && (
            <div className="approval-result-output">
              <small>
                {tool.status === "failed" ? "错误输出" : "工具结果"}
              </small>
              <pre>
                {typeof tool.output === "string"
                  ? tool.output
                  : JSON.stringify(tool.output, null, 2)}
              </pre>
            </div>
          )}
        </div>
      </details>
    );
  return (
    <section className="approval-card" aria-label={`${label}，需要确认`}>
      <header>
        <Icon size={16} aria-hidden="true" />
        <span>{category}</span>
      </header>
      <p className="approval-summary">{question}</p>
      <ApprovalOperation input={request.input} compact />
      <footer>
        {parameters}
        <div className="approval-actions">
          <button disabled={disabled || busy} onClick={() => onDecision(false)}>
            拒绝
          </button>
          <button
            className="primary"
            disabled={disabled || busy}
            onClick={() => onDecision(true)}
          >
            {busy ? "处理中…" : "允许一次"}
          </button>
        </div>
      </footer>
    </section>
  );
}
