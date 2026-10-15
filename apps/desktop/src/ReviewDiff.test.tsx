// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { ReviewDiff } from "./ReviewDiff";
afterEach(cleanup);
it("shows the code hunk and EOF marker without repeating raw file headers", () => {
  render(
    <ReviewDiff
      split={false}
      content={
        "diff --git a/x b/x\nindex abc..def\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n"
      }
    />,
  );
  expect(screen.queryByText(/diff --git/)).toBeNull();
  expect(screen.getByText(/No newline/)).toBeTruthy();
  expect(screen.getByText("+new")).toBeTruthy();
});
it("keeps metadata visible for binary or permission-only diffs", () => {
  render(
    <ReviewDiff
      split
      content={
        "diff --git a/x b/x\nold mode 100644\nnew mode 100755\n@@ -1 +1 @@\n-old\n+new\n"
      }
    />,
  );
  expect(screen.getByText("new mode 100755")).toBeTruthy();
});
it("keeps all received lines available beyond the first render batch", async () => {
  const { fireEvent } = await import("@testing-library/react");
  render(
    <ReviewDiff
      split={false}
      content={`@@ -0,0 +1,4100 @@\n${Array.from({ length: 4100 }, (_, i) => `+line ${i + 1}`).join("\n")}`}
    />,
  );
  expect(screen.queryByText("+line 4100")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /显示更多差异/ }));
  expect(screen.getByText("+line 4100")).toBeTruthy();
  expect(screen.queryByText(/仅显示前 4000/)).toBeNull();
});
it("shows real hunk positions and escaped syntax without converting source into HTML", () => {
  const { container } = render(
    <ReviewDiff
      split={false}
      path="a.rs"
      content={
        '@@ -526 +527 @@\n+pub struct Child {\n+  text: "<script>bad</script>",\n'
      }
    />,
  );
  expect(screen.getByText(/525 行未修改/)).toBeTruthy();
  expect(container.querySelector(".syntax-keyword")?.textContent).toBe("pub");
  expect(container.querySelector("script")).toBeNull();
  expect(container.textContent).toContain("<script>bad</script>");
});
