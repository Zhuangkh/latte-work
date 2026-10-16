import { expect, it } from "vitest";
import type { Event, EventKind } from "./protocol";
import { artifactPath, taskInfo, createTaskInfoReader } from "./taskInfoModel";
const events = (...items: EventKind[]): Event[] =>
  items.map((event, i) => ({ seq: i + 1, session_id: "s", at: i, event }));
it("includes references in handed-off input before its execution starts", () => {
  const input = {
    request_id: "queued",
    text: "[参考文档](https://example.com/docs) [报告](/project/report.md)",
  };
  const sent = events({ kind: "queued_message_sent", ...input });
  const before = taskInfo(sent, "s", "/project");
  expect(before.resources).toHaveLength(2);
  const after = taskInfo(
    [...sent, { ...events({ kind: "user", ...input })[0], seq: 2 }],
    "s",
    "/project",
  );
  expect(after.resources).toEqual(before.resources);
});
it("uses successful todo results, replaces snapshots, and ignores rejected or duplicated calls", () => {
  const history = events(
    {
      kind: "tool",
      id: "a",
      name: "TodoWrite",
      input: { todos: [{ content: "Implement", status: "pending" }] },
    },
    { kind: "tool_result", id: "a", content: "ok", is_error: false },
    {
      kind: "tool",
      id: "b",
      name: "TodoWrite",
      input: { todos: [{ content: "Wrong", status: "completed" }] },
    },
    { kind: "tool_result", id: "b", content: "denied", is_error: true },
    {
      kind: "tool",
      id: "c",
      name: "TodoWrite",
      input: {
        todos: [
          { content: "Implement", status: "completed" },
          { content: "Verify", status: "in_progress" },
        ],
      },
    },
  );
  expect(taskInfo(history, "s").todos.map((item) => item.title)).toEqual([
    "Implement",
  ]);
  history.push(
    ...events({
      kind: "tool_result",
      id: "c",
      content: "ok",
      is_error: false,
    }).map((e) => ({ ...e, seq: 6 })),
  );
  expect(taskInfo([...history, history[0]], "s")).toMatchObject({
    todos: [
      { title: "Implement", status: "completed" },
      { title: "Verify", status: "in_progress" },
    ],
    tools: [["TodoWrite", 3]],
  });
});
it("links task creation IDs to confirmed updates and deletion", () => {
  const history = events(
    { kind: "tool", id: "1", name: "TaskCreate", input: { subject: "Review" } },
    {
      kind: "tool_result",
      id: "1",
      content: [{ type: "text", text: "Task #7 created successfully: Review" }],
      is_error: false,
    },
    {
      kind: "tool",
      id: "2",
      name: "TaskUpdate",
      input: { taskId: "7", status: "completed" },
    },
    { kind: "tool_result", id: "2", content: "ok", is_error: false },
  );
  expect(taskInfo(history, "s").todos).toEqual([
    { id: "7", title: "Review", status: "completed" },
  ]);
  history.push(
    ...events(
      {
        kind: "tool",
        id: "3",
        name: "TaskUpdate",
        input: { taskId: "7", status: "deleted" },
      },
      { kind: "tool_result", id: "3", content: "ok", is_error: false },
    ).map((e) => ({ ...e, seq: e.seq + 4 })),
  );
  expect(taskInfo(history, "s").todos).toEqual([]);
});
it("assembles streaming links, collects explicit outputs, and excludes code examples and unsafe URLs", () => {
  const history = events(
    {
      kind: "user",
      request_id: "r",
      text: "See https://example.com/docs. [Input](/project/input.txt)",
    },
    { kind: "text", text: "[Guide](https://example.com/" },
    {
      kind: "text",
      text: "guide) [Report](</project/My Report.md:12>)\n```sh\nhttps://example.com/example\n```\nhttps://user:secret@example.com/private",
    },
    {
      kind: "tool",
      id: "w",
      name: "Write",
      input: { file_path: "/project/output.md" },
    },
    { kind: "tool_result", id: "w", content: "ok", is_error: false },
    {
      kind: "tool",
      id: "bad",
      name: "Write",
      input: { file_path: "/project/failed.md" },
    },
    { kind: "tool_result", id: "bad", content: "no", is_error: true },
    {
      kind: "tool",
      id: "sk",
      name: "Skill",
      input: { skill: "review-code-changes" },
    },
  );
  const data = taskInfo(
    [
      ...history,
      {
        ...history[0],
        seq: 99,
        session_id: "other",
        event: { kind: "text", text: "https://wrong.example" },
      },
    ],
    "s",
  );
  expect(data.links).toEqual([
    { url: "https://example.com/docs", title: "https://example.com/docs" },
    { url: "https://example.com/guide", title: "Guide" },
  ]);
  expect(data.files.map((file) => file.path)).toEqual([
    "/project/input.txt",
    "/project/My Report.md",
    "/project/output.md",
  ]);
  expect(data.skills).toEqual([["review-code-changes", 1]]);
  expect(taskInfo(history, "other").files).toEqual([]);
});
it("normalizes line references and bounds inventories", () => {
  expect(artifactPath("/project/main.ts:12:3")).toBe("/project/main.ts");
  expect(artifactPath("javascript:alert(1)")).toBeNull();
  expect(artifactPath("//example.com/path")).toBeNull();
  const data = taskInfo(
    events({
      kind: "text",
      text: Array.from(
        { length: 130 },
        (_, i) => `https://example.com/${i}`,
      ).join(" "),
    }),
    "s",
  );
  expect(data.links).toHaveLength(128);
  expect(data.truncated).toBe(true);
});

it("groups generated files as products and inputs, read files and web links as resources", () => {
  const marker = "\n\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n";
  const data = taskInfo(
    events(
      {
        kind: "user",
        request_id: "r",
        text: `See https://example.com${marker}${JSON.stringify([
          { path: "/project/input.pdf", type: "file" },
          { path: "/project/output.md", type: "file" },
        ])}`,
      },
      {
        kind: "tool",
        id: "read",
        name: "Read",
        input: { file_path: "/project/source.ts" },
      },
      { kind: "tool_result", id: "read", content: "ok", is_error: false },
      {
        kind: "tool",
        id: "write",
        name: "Write",
        input: { file_path: "/project/output.md" },
      },
      { kind: "tool_result", id: "write", content: "ok", is_error: false },
      {
        kind: "text",
        text: "[Report](/project/output.md) [Reference](https://example.com/docs)",
      },
    ),
    "s",
    "/project",
  );
  expect(data.products).toEqual([
    { kind: "file", path: "output.md", name: "Report", directory: false },
  ]);
  expect(
    data.resources.map((item) => (item.kind === "file" ? item.path : item.url)),
  ).toEqual(["input.pdf", "https://example.com/", "https://example.com/docs"]);
});

it("keeps documents and attachments across all sources without code consuming the inventory bound", () => {
  const writes: EventKind[] = Array.from({ length: 140 }, (_, i) => [
    {
      kind: "tool" as const,
      id: `w${i}`,
      name: "Write",
      input: { file_path: `/project/src/file${i}.py` },
    },
    {
      kind: "tool_result" as const,
      id: `w${i}`,
      content: "ok",
      is_error: false,
    },
  ]).flat();
  const data = taskInfo(
    events(
      ...writes,
      {
        kind: "user",
        request_id: "r",
        text: "[Code](src/main.rs:12) [Config](config.json) [Notebook](demo.ipynb) [Folder](src/) [Input](docs/INPUT.PDF) [Photo](image.png)",
      },
      {
        kind: "tool",
        id: "read",
        name: "Read",
        input: { file_path: "/project/src/main.ts" },
      },
      { kind: "tool_result", id: "read", content: "ok", is_error: false },
      {
        kind: "tool",
        id: "doc",
        name: "Write",
        input: { file_path: "/project/report.html" },
      },
      { kind: "tool_result", id: "doc", content: "ok", is_error: false },
      {
        kind: "turn_changes",
        changes: {
          request_id: "r",
          interrupted: false,
          background_pending: false,
          undo: "ready",
          summary: {
            entries: [
              "src/main.ts",
              "settings.yaml",
              "README",
              "docs/summary.md",
              "results.csv",
            ].map((path) => ({ path, status: "M", added: 1, removed: 0 })),
            added: 5,
            removed: 0,
            binary_files: 0,
            truncated: false,
            baseline_at: 1,
            unavailable: null,
          },
        },
      },
    ),
    "s",
    "/project",
  );
  expect(
    data.products.map((item) => item.kind === "file" && item.path),
  ).toEqual(["report.html", "README", "docs/summary.md", "results.csv"]);
  expect(
    data.resources.map((item) => item.kind === "file" && item.path),
  ).toEqual(["docs/INPUT.PDF", "image.png"]);
  expect(data.files).toHaveLength(6);
  expect(data.truncated).toBe(false);
});

it("retains an attachment's original document name when storage uses an extensionless ID", () => {
  const marker = "\n\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n";
  const data = taskInfo(
    events({
      kind: "user",
      request_id: "r",
      text: `Attachments${marker}${JSON.stringify([
        { path: "/tmp/attachment-id", name: "Project brief.pdf", type: "file" },
        { path: "/project/code.py", name: "Disguised.md", type: "file" },
        { path: "/project/reports", name: "Reports.pdf", type: "directory" },
      ])}`,
    }),
    "s",
    "/project",
  );
  expect(data.files).toEqual([
    { path: "/tmp/attachment-id", name: "Project brief.pdf", directory: false },
  ]);
});

it("keeps TaskCreate records when TodoWrite replaces only its own snapshot", () => {
  const history = events(
    {
      kind: "tool",
      id: "task",
      name: "TaskCreate",
      input: { subject: "Native task" },
    },
    {
      kind: "tool_result",
      id: "task",
      content: { taskId: "7" },
      is_error: false,
    },
    {
      kind: "tool",
      id: "todos-a",
      name: "TodoWrite",
      input: { todos: [{ content: "Old todo", status: "pending" }] },
    },
    { kind: "tool_result", id: "todos-a", content: "ok", is_error: false },
    {
      kind: "tool",
      id: "todos-b",
      name: "TodoWrite",
      input: { todos: [{ content: "Current todo", status: "completed" }] },
    },
    { kind: "tool_result", id: "todos-b", content: "ok", is_error: false },
  );
  expect(taskInfo(history, "s").todos).toEqual([
    { id: "7", title: "Native task", status: "pending" },
    { id: "todo:0", title: "Current todo", status: "completed" },
  ]);
});

it("parses each new stream fragment once and rebuilds after history or record replacement", () => {
  const read = createTaskInfoReader();
  const history: Event[] = [];
  let reads = 0;
  for (let i = 0; i < 200; i++) {
    history.push({
      seq: i + 1,
      at: i,
      session_id: "s",
      event: {
        kind: "text",
        get text() {
          reads++;
          return `[Doc](https://example.com/${i})\n`;
        },
      },
    });
    expect(read(history, "s", undefined, false).links).toEqual([]);
  }
  expect(reads).toBe(200);
  history.push({
    seq: 201,
    at: 201,
    session_id: "s",
    event: { kind: "state", status: "completed", message: null },
  });
  expect(read(history, "s").links).toHaveLength(128);
  expect(reads).toBe(200);
  const older = {
    seq: 0,
    at: 0,
    session_id: "s",
    event: {
      kind: "user" as const,
      request_id: "older",
      text: "[Earlier](/tmp/earlier.md)",
    },
  };
  expect(read([older, ...history], "s").resources[0]).toMatchObject({
    path: "/tmp/earlier.md",
  });
  const replacement = events({ kind: "text", text: "[New](/tmp/new.md)" });
  expect(read(replacement, "s").files).toEqual([
    { path: "/tmp/new.md", name: "New", directory: false },
  ]);
  expect(read(replacement, "other").files).toEqual([]);
});

it("does not keep a temporary partial URL after a later stream fragment extends it", () => {
  const read = createTaskInfoReader();
  const history = events({ kind: "text", text: "https://example.com/part" });
  expect(read(history, "s").links[0].url).toBe("https://example.com/part");
  history.push({
    seq: 2,
    at: 2,
    session_id: "s",
    event: { kind: "text", text: "-two" },
  });
  expect(read(history, "s").links).toEqual([
    {
      url: "https://example.com/part-two",
      title: "https://example.com/part-two",
    },
  ]);
});

it("treats unclosed and repeated fence markers as code instead of rescanning the remaining message", () => {
  const body =
    "[Visible](https://example.com/visible)\n~~~\n" +
    Array.from(
      { length: 2000 },
      (_, i) => `\`\`\`lang-${i}\nhttps://example.com/code-${i}`,
    ).join("\n");
  expect(taskInfo(events({ kind: "text", text: body }), "s").links).toEqual([
    { url: "https://example.com/visible", title: "Visible" },
  ]);
});
