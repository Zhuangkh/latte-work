import { WebLink } from "./LinkHandling";
import { copyText } from "./clipboard";
import {
  Children,
  isValidElement,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { message } from "./api";

export function CopyButton({
  text,
  label,
  iconOnly = false,
}: {
  text: string;
  label: string;
  iconOnly?: boolean;
}) {
  const [state, setState] = useState("");
  useEffect(() => setState(""), [text]);
  return (
    <span className="copy-action">
      <button
        aria-label={state === "已复制" ? state : label}
        title={state || label}
        onClick={() => {
          void copyText(text).then(
            () => setState("已复制"),
            (e) => setState(`复制失败：${message(e)}`),
          );
        }}
      >
        {state === "已复制" ? <Check size={14} /> : <Copy size={14} />}
        {!iconOnly && (state === "已复制" ? state : label)}
      </button>
      {state.startsWith("复制失败") && <span role="alert">{state}</span>}
    </span>
  );
}
function CodeBlock({ children }: { children?: ReactNode }) {
  const code = Children.toArray(children).find(isValidElement);
  const props = code?.props as
    { className?: string; children?: string } | undefined;
  return (
    <div className="code-block">
      <div className="code-block-heading">
        <span>{props?.className?.replace("language-", "") || "代码"}</span>
        <CopyButton
          label="复制代码"
          text={String(props?.children ?? "").replace(/\n$/, "")}
        />
      </div>
      <pre>{children}</pre>
    </div>
  );
}
export function MessageContent({ text }: { text: string }) {
  return (
    <>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{ pre: CodeBlock, a: WebLink }}
      >
        {text}
      </ReactMarkdown>
    </>
  );
}
