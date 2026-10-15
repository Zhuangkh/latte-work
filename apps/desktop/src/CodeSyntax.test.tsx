// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { CodeSyntax } from "./CodeSyntax";

afterEach(cleanup);
it.each([
  [
    "a.rs",
    "// first comment\nlet value = 1;\n// last comment",
    ["// first comment", "// last comment"],
  ],
  [
    "a.py",
    "# first comment\nvalue = 1\n# last comment",
    ["# first comment", "# last comment"],
  ],
  ["a.ts", "/* block\ncomment */\nconst value = 1;", ["/* block\ncomment */"]],
])(
  "highlights complete comments across lines in %s",
  (path, text, comments) => {
    const { container } = render(<CodeSyntax text={text} path={path} />);
    expect(
      [...container.querySelectorAll(".syntax-comment")].map(
        (e) => e.textContent,
      ),
    ).toEqual(comments);
    expect(container.textContent).toBe(text);
    expect(container.querySelector(".syntax-number")?.textContent).toBe("1");
  },
);
it("preserves escaped strings and unfamiliar files as literal text", () => {
  const text = 'const text = "// <script>literal</script>";';
  const { container, rerender } = render(
    <CodeSyntax text={text} path="a.ts" />,
  );
  expect(container.querySelector(".syntax-comment")).toBeNull();
  expect(container.querySelector("script")).toBeNull();
  expect(container.textContent).toBe(text);
  rerender(<CodeSyntax text={text} path="a.txt" />);
  expect(container.querySelector("span")).toBeNull();
  expect(container.textContent).toBe(text);
});
