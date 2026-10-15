import { CopyButton } from "./MessageContent";
import { MessageTimestamp } from "./MessageTimestamp";
import { useMemo } from "react";
import { FileText, Folder } from "lucide-react";
import { AttachmentImage } from "./AttachmentImage";
import { referenceKind, referenceMessage } from "./referenceMessage";

export function UserMessage({
  text,
  at,
  hostId = "local",
  projectId,
  sessionId,
  connected = false,
  active = true,
}: {
  text: string;
  at: number;
  hostId?: string;
  projectId?: string;
  sessionId?: string;
  connected?: boolean;
  active?: boolean;
}) {
  const content = useMemo(() => referenceMessage(text), [text]);
  return (
    <div className="user-message-group">
      {content.references.length > 0 && (
        <div className="user-message-attachments" aria-label="消息附件">
          {content.references.map((reference, index) =>
            !reference.directory && referenceKind(reference) === "image" ? (
              <AttachmentImage
                key={`${reference.path}:${index}`}
                reference={reference}
                variant="thumbnail"
                hostId={hostId}
                projectId={projectId}
                sessionId={sessionId}
                connected={connected}
                active={active}
              />
            ) : (
              <span
                key={`${reference.path}:${index}`}
                className="user-file-reference"
                title={reference.path}
              >
                {reference.directory ? (
                  <Folder size={16} />
                ) : (
                  <FileText size={16} />
                )}
                <span>{reference.name}</span>
              </span>
            ),
          )}
        </div>
      )}
      {content.text && <div className="user-message">{content.text}</div>}
      <div className="user-message-meta">
        <MessageTimestamp at={at} label="发送时间" />
        <CopyButton text={text} label="复制消息" iconOnly />
      </div>
    </div>
  );
}
