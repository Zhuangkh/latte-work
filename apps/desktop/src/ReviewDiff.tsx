import { useMemo, useState } from "react";
import { parseDiff, splitDiff } from "./gitDiff";
import { CodeSyntax } from "./CodeSyntax";
export function ReviewDiff({
  content,
  split,
  wrap = false,
  path,
}: {
  content: string;
  split: boolean;
  wrap?: boolean;
  path?: string;
}) {
  const [displayed, showMore] = useState(4000);
  const parsed = useMemo(
    () => parseDiff(content, Number.POSITIVE_INFINITY),
    [content],
  );
  const lines = useMemo(
    () =>
      parsed.lines.some((line) => line.kind === "hunk")
        ? parsed.lines.filter(
            (line) =>
              line.kind !== "meta" ||
              !/^(diff --git |index |--- |\+\+\+ )/.test(line.text),
          )
        : parsed.lines,
    [parsed.lines],
  );
  const headings = useMemo(() => {
    const labels = new Map<string, string>();
    let previousOld = 0;
    for (const line of lines) {
      if (line.kind === "hunk") {
        const hunk = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)/.exec(
          line.text,
        );
        if (hunk) {
          const unchanged = Math.max(0, Number(hunk[1]) - previousOld - 1);
          labels.set(
            line.text,
            `${unchanged ? `${unchanged} 行未修改 · ` : ""}原文件第 ${hunk[1]} 行 · 新文件第 ${hunk[3]} 行${hunk[5] ? ` ·${hunk[5]}` : ""}`,
          );
        }
      }
      if (line.old !== null) previousOld = line.old;
    }
    return labels;
  }, [lines]);
  const shown = lines.slice(0, displayed);
  const rows = useMemo(() => splitDiff(shown), [lines, displayed]);
  function code(text: string) {
    return <CodeSyntax text={text || " "} path={path} />;
  }
  function contextHeading(text: string) {
    return headings.get(text) ?? text;
  }
  return (
    <>
      <div
        className={`review-diff${split ? " split" : ""}${wrap ? " wrap" : ""}`}
        role="region"
        aria-label={split ? "并排文件差异" : "统一文件差异"}
        tabIndex={0}
      >
        {split
          ? rows.map((row, i) =>
              row.heading ? (
                <div
                  className={`review-diff-heading ${row.heading.kind}`}
                  key={i}
                >
                  <code>{contextHeading(row.heading.text)}</code>
                </div>
              ) : (
                <div className="review-split-row" key={i}>
                  <div
                    className={`review-diff-cell ${row.left?.kind ?? "blank"}`}
                  >
                    <span aria-hidden="true">{row.left?.old}</span>
                    <code>{code(row.left?.text ?? "")}</code>
                  </div>
                  <div
                    className={`review-diff-cell ${row.right?.kind ?? "blank"}`}
                  >
                    <span aria-hidden="true">{row.right?.next}</span>
                    <code>{code(row.right?.text ?? "")}</code>
                  </div>
                </div>
              ),
            )
          : shown.map((line, i) => (
              <div className={`review-unified-row ${line.kind}`} key={i}>
                <span
                  aria-hidden="true"
                  title={`原文件 ${line.old ?? "—"} · 新文件 ${line.next ?? "—"}`}
                >
                  {line.kind === "removed" ? line.old : (line.next ?? line.old)}
                </span>
                <code>
                  {line.kind === "added"
                    ? "+"
                    : line.kind === "removed"
                      ? "−"
                      : " "}
                  {line.kind === "hunk"
                    ? contextHeading(line.text)
                    : code(line.text)}
                </code>
              </div>
            ))}
      </div>
      {displayed < lines.length && (
        <button
          className="review-show-more"
          onClick={() => showMore((n) => n + 4000)}
        >
          显示更多差异（还有 {lines.length - displayed} 行）
        </button>
      )}
    </>
  );
}
