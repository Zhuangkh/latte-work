import { Fragment } from "react";
import { ChevronRight } from "lucide-react";
import { activityTranscript, type ActivityItem } from "./activity";
import { MessageContent } from "./MessageContent";
import { WorkingStatus } from "./WorkingStatus";
import { ReplyActions } from "./ReplyActions";
import { ToolRow, ToolGroup } from "./ToolActivity";
import { ProcessDisclosure } from "./TurnTranscript";
import type { Event, Subagent } from "./protocol";

function renderItem(item: ActivityItem) {
  if (item.type === "assistant")
    return (
      <article className="markdown">
        <MessageContent text={item.text} />
      </article>
    );
  if (item.type === "tool") return <ToolRow tool={item} />;
  if (item.type === "tools") return <ToolGroup tools={item.tools} />;
  if (item.type === "event" && item.value.kind === "notice")
    return <p className="subagent-empty">{item.value.text}</p>;
  return null;
}

export function SubagentTranscript({
  task,
  events,
  elapsed,
  incomplete = false,
  readingMore = false,
  prompt,
}: {
  task: Subagent;
  events: Event[];
  elapsed: string;
  incomplete?: boolean;
  readingMore?: boolean;
  prompt?: string | null;
}) {
  const allItems = activityTranscript(events);
  const instruction =
    prompt || allItems.find((item) => item.type === "user")?.text;
  const items = allItems.filter((item) => item.type !== "user");
  if (!["running", "paused"].includes(task.status)) {
    for (const item of items) {
      const tools =
        item.type === "tool" ? [item] : item.type === "tools" ? item.tools : [];
      for (const tool of tools)
        if (tool.status === "pending") tool.status = "unconfirmed";
    }
  }
  const completed = task.status === "completed" && !readingMore;
  const lastAssistant = [...items]
    .reverse()
    .find((item) => item.type === "assistant");
  const finalIndex = lastAssistant ? items.indexOf(lastAssistant) : -1;
  const hasFinalText =
    completed &&
    finalIndex >= 0 &&
    !items
      .slice(finalIndex + 1)
      .some((item) => item.type === "tool" || item.type === "tools");
  const final = completed
    ? hasFinalText && lastAssistant?.type === "assistant"
      ? lastAssistant.text
      : task.summary
    : null;
  const process = hasFinalText ? items.slice(0, finalIndex) : items;
  return (
    <div className="subagent-transcript">
      {instruction && (
        <details className="turn-process subagent-instruction">
          <summary>
            任务指令
            <ChevronRight size={14} aria-hidden="true" />
          </summary>
          <div className="markdown">
            <MessageContent text={instruction} />
          </div>
        </details>
      )}
      {process.length > 0 &&
        (completed ? (
          <ProcessDisclosure key={`${task.id}:${task.status}`}>
            <summary>
              <span>{elapsed ? `用时 ${elapsed}` : "执行过程"}</span>
              <ChevronRight size={14} aria-hidden="true" />
            </summary>
            {() => (
              <div className="turn-process-content">
                {process.map((item) => (
                  <Fragment key={item.key}>{renderItem(item)}</Fragment>
                ))}
              </div>
            )}
          </ProcessDisclosure>
        ) : (
          process.map((item) => (
            <Fragment key={item.key}>{renderItem(item)}</Fragment>
          ))
        ))}
      {final ? (
        <>
          <article className="markdown subagent-result">
            <MessageContent text={final} />
          </article>
          <ReplyActions text={final} at={task.updated_at} />
        </>
      ) : !readingMore && task.summary ? (
        <article className="markdown subagent-result">
          <MessageContent text={task.summary} />
        </article>
      ) : completed ? (
        <p className="subagent-empty">未收到最终输出</p>
      ) : null}
      {readingMore && <WorkingStatus label="正在读取完整记录…" animated />}
      {incomplete && (
        <p className="subagent-empty">
          部分原始记录暂不可读取，当前保留已保存的内容。
        </p>
      )}
    </div>
  );
}
