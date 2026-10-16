import type { Event, JsonValue } from "./protocol";
import { referenceMessage } from "./referenceMessage";
import { webLink } from "./browserLinks";
import { isTaskResourceFile } from "./taskResourceFiles";

export interface InfoFile {
  path: string;
  name: string;
  directory: boolean;
}
export type InfoResource =
  ({ kind: "file" } & InfoFile) | { kind: "link"; url: string; title: string };
export interface InfoTodo {
  id: string;
  title: string;
  status: string;
}
const record = (
  value: JsonValue | undefined,
): Record<string, JsonValue | undefined> =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};
const text = (value: JsonValue | undefined) =>
  typeof value === "string" ? value : "";
const statuses = new Set(["pending", "in_progress", "completed"]);

export function artifactPath(value: string): string | null {
  let path = value.replace(/^<|>$/g, "");
  try {
    path = decodeURI(path);
  } catch {
    /* Preserve literal filenames. */
  }
  path = path.replace(/(?:#L\d+(?:-L?\d+)?|:\d+(?::\d+)?)$/, "");
  if (
    !path ||
    path.length > 4096 ||
    /[\x00-\x1f]/.test(path) ||
    /^[a-z][a-z\d+.-]*:/i.test(path) ||
    path.startsWith("//") ||
    path.startsWith("#")
  )
    return null;
  return path;
}

const newState = () => ({
  links: new Map<string, { url: string; title: string }>(),
  files: new Map<string, InfoFile>(),
  products: new Map<string, InfoResource>(),
  resources: new Map<string, InfoResource>(),
  skills: new Map<string, number>(),
  tools: new Map<string, number>(),
  calls: new Map<string, Extract<Event["event"], { kind: "tool" }>>(),
  results: new Set<string>(),
  todos: new Map<string, InfoTodo>(),
});
function collector(projectPath?: string, state = newState()) {
  const {
    links,
    files,
    products,
    resources,
    skills,
    tools,
    calls,
    results,
    todos,
  } = state;
  const file = (
    value: string,
    title?: string,
    product = false,
    directory = false,
    attachmentName?: string,
  ) => {
    let path = artifactPath(value);
    if (
      path &&
      projectPath &&
      path.startsWith(`${projectPath.replace(/\/$/, "")}/`)
    )
      path = path.slice(projectPath.replace(/\/$/, "").length + 1);
    if (!path || !isTaskResourceFile(path, directory, attachmentName)) return;
    path = path.replace(/^\.\//, "");
    const entry: InfoFile = {
      path,
      name: title || files.get(path)?.name || path.split("/").at(-1) || path,
      directory,
    };
    if (files.size < 129 || files.has(path)) files.set(path, entry);
    const key = `file:${path}`;
    if (product) resources.delete(key);
    const target = product || products.has(key) ? products : resources;
    if (target.size < 129 || target.has(key))
      target.set(key, { kind: "file", ...entry });
  };
  const link = (value: string, title?: string) => {
    const url = webLink(value);
    if (url && (links.size < 129 || links.has(url))) {
      const entry = { url, title: title || links.get(url)?.title || url };
      links.set(url, entry);
      if (resources.size < 129 || resources.has(`link:${url}`))
        resources.set(`link:${url}`, { kind: "link", ...entry });
    }
  };
  const scan = (body: string, user: boolean) => {
    if (user) {
      const parsed = referenceMessage(body);
      for (const entry of parsed.references)
        file(entry.path, entry.name, false, entry.directory, entry.name);
      body = parsed.text;
    }
    // Ignore fenced examples; inline code can contain actual linked file paths.
    let fence: string | undefined;
    const prose = body
      .split("\n")
      .filter((line) => {
        const marker = line.match(/^ {0,3}(`{3,}|~{3,})/);
        if (fence) {
          if (
            marker &&
            marker[1][0] === fence[0] &&
            marker[1].length >= fence.length &&
            line.slice(marker[0].length).trim() === ""
          )
            fence = undefined;
          return false;
        }
        if (marker) {
          fence = marker[1];
          return false;
        }
        return true;
      })
      .join("\n");
    const rest = prose.replace(
      /!?\[([^\]]*)\]\(\s*(<[^>]+>|[^\s)]+)(?:\s+["'][^\n]*?["'])?\s*\)/g,
      (_match, title: string, target: string) => {
        const value = target.replace(/^<|>$/g, "");
        if (webLink(value)) link(value, title);
        else file(value, title);
        return "";
      },
    );
    for (const match of rest.matchAll(
      /https?:\/\/[^\s<>`"'\[\]）。，；！？]+/g,
    )) {
      let url = match[0].replace(/[.,;!?]+$/, "");
      while (
        url.endsWith(")") &&
        (url.match(/\)/g)?.length ?? 0) > (url.match(/\(/g)?.length ?? 0)
      )
        url = url.slice(0, -1);
      link(url);
    }
    for (const match of rest.matchAll(/`((?:\/|\.\/)[^`\n]+)`/g))
      if (/\/[^/]+\.[^/]+$/.test(match[1])) file(match[1]);
  };
  // Streaming fragments are contiguous text events, so assemble before parsing links.
  let body = "",
    bodyKind = "";
  const flush = () => {
    if (body) scan(body, bodyKind === "user");
    body = "";
  };
  const append = (event: Event["event"]) => {
    if (
      event.kind === "text" ||
      event.kind === "user" ||
      event.kind === "queued_message_sent"
    ) {
      const kind = event.kind === "text" ? "text" : "user";
      if (kind !== bodyKind || kind === "user") flush();
      bodyKind = kind;
      body += event.text;
      return;
    }
    flush();
    if (event.kind === "tool") {
      if (calls.has(event.id)) return;
      calls.set(event.id, event);
      tools.set(event.name, (tools.get(event.name) ?? 0) + 1);
      if (event.name === "Skill") {
        const name = text(record(event.input).skill);
        if (name) skills.set(name, (skills.get(name) ?? 0) + 1);
      }
    }
    if (event.kind === "tool_result" && !results.has(event.id)) {
      results.add(event.id);
      const call = calls.get(event.id);
      if (!call || event.is_error) return;
      const input = record(call.input);
      if (call.name === "TodoWrite" && Array.isArray(input.todos)) {
        for (const id of todos.keys())
          if (id.startsWith("todo:")) todos.delete(id);
        input.todos.forEach((value, i) => {
          const todo = record(value);
          const title = text(todo.content);
          if (title && statuses.has(text(todo.status)))
            todos.set(`todo:${i}`, {
              id: `todo:${i}`,
              title,
              status: text(todo.status),
            });
        });
      }
      if (call.name === "TaskCreate") {
        const output = record(event.content);
        const content =
          typeof event.content === "string"
            ? event.content
            : Array.isArray(event.content)
              ? event.content.map((part) => text(record(part).text)).join("\n")
              : "";
        const id =
          text(output.taskId) ||
          text(record(output.task ?? null).id) ||
          content.match(/Task #(\d+) created successfully/)?.[1];
        if (id && text(input.subject))
          todos.set(id, { id, title: text(input.subject), status: "pending" });
      }
      if (call.name === "TaskUpdate") {
        const id = text(input.taskId),
          old = todos.get(id);
        if (text(input.status) === "deleted") todos.delete(id);
        else if (old)
          todos.set(id, {
            ...old,
            title: text(input.subject) || old.title,
            status: statuses.has(text(input.status))
              ? text(input.status)
              : old.status,
          });
      }
      if (["Write", "Edit", "MultiEdit", "NotebookEdit"].includes(call.name)) {
        const path = text(input.file_path) || text(input.notebook_path);
        if (path) file(path, undefined, true);
      }
      if (call.name === "Read") {
        const path = text(input.file_path);
        if (path) file(path);
      }
    }
    if (event.kind === "turn_changes")
      for (const entry of event.changes.summary.entries)
        if (entry.status !== "D" && entry.status !== "deleted")
          file(entry.path, undefined, true);
  };
  const limited = <T>(values: Iterable<T>) => [...values].slice(0, 128);
  const snapshot = () => ({
    products: limited(products.values()),
    resources: limited(resources.values()),
    todos: limited(todos.values()),
    skills: limited(skills),
    tools: limited(tools),
    links: limited(links.values()),
    files: limited(files.values()),
    truncated: [todos, skills, tools, links, files, products, resources].some(
      (items) => items.size > 128,
    ),
  });
  return {
    append,
    snapshot(includePending = true): ReturnType<typeof snapshot> {
      if (!body || !includePending) return snapshot();
      // Project the unfinished message into temporary maps: partial URLs must
      // not remain as phantom resources when the next fragment extends them.
      const visible = collector(projectPath, {
        ...state,
        links: new Map(links),
        files: new Map(files),
        products: new Map(products),
        resources: new Map(resources),
      });
      visible.scanPending(body, bodyKind === "user");
      return visible.snapshot(false);
    },
    scanPending: scan,
  };
}

/** Reuse parsed records for an append-only stream; a prepended history page or
 * replaced record resets the projection. Only completed text blocks are committed. */
export function createTaskInfoReader() {
  let previous: Event[] = [];
  let context = "";
  let model = collector();
  return (
    events: Event[],
    sessionId?: string,
    projectPath?: string,
    includePending = true,
  ) => {
    const scope = JSON.stringify([sessionId, projectPath]);
    let ordered = events.filter((entry) => entry.session_id === sessionId);
    if (ordered.some((entry, i) => i > 0 && entry.seq < ordered[i - 1].seq))
      ordered = ordered.sort((a, b) => a.seq - b.seq);
    if (
      scope !== context ||
      previous.length > ordered.length ||
      !previous.every((entry, i) => entry === ordered[i])
    ) {
      model = collector(projectPath);
      previous = [];
      context = scope;
    }
    for (let i = previous.length; i < ordered.length; i++)
      model.append(ordered[i].event);
    previous = ordered;
    return model.snapshot(includePending);
  };
}

/** A standalone projection for callers without an incremental event stream. */
export function taskInfo(
  events: Event[],
  sessionId?: string,
  projectPath?: string,
) {
  return createTaskInfoReader()(events, sessionId, projectPath);
}
