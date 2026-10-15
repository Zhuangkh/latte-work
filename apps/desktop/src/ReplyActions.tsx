import { CopyButton } from "./MessageContent";
import { MessageTimestamp } from "./MessageTimestamp";

export function ReplyActions({ text, at }: { text: string; at?: number }) {
  return (
    <div className="final-reply-actions">
      <CopyButton text={text} label="复制回复" iconOnly />
      <MessageTimestamp at={at} label="回复时间" />
    </div>
  );
}
