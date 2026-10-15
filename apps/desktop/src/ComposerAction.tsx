import { ArrowUp, LoaderCircle, Square } from "lucide-react";
export function ComposerAction({
  active = false,
  sending = false,
  disabled = false,
  label,
  onClick,
}: {
  active?: boolean;
  sending?: boolean;
  disabled?: boolean;
  label?: string;
  onClick?: () => void;
}) {
  return (
    <button
      className={`send${active ? " stop" : ""}`}
      aria-label={
        sending ? "正在发送任务" : active ? "停止任务" : (label ?? "发送任务")
      }
      title={
        sending ? "正在发送任务" : active ? "停止任务" : (label ?? "发送任务")
      }
      disabled={disabled || sending}
      onClick={onClick}
    >
      {sending ? (
        <LoaderCircle size={17} className="spin" />
      ) : active ? (
        <Square size={14} fill="currentColor" />
      ) : (
        <ArrowUp size={19} />
      )}
    </button>
  );
}
