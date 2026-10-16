import type { Session, Event, RecentSession } from "./protocol";
import type { HostedProject } from "./projectCatalog";
import { transcript } from "./transcript";

export type HostedSession = Session & { hostId: string };
export function sessionKey(hostId: string, id: string) {
  return JSON.stringify([hostId, id]);
}
export function recentSessions(
  cache: Record<string, RecentSession[]>,
  projects: HostedProject[],
  hostId: string,
  current: Session[],
  pinned: HostedSession[],
): (HostedSession & { updated_at: number })[] {
  const known = new Set(
    projects.map((project) => sessionKey(project.hostId, project.id)),
  );
  const live = new Map(
    [...pinned, ...current.map((session) => ({ ...session, hostId }))].map(
      (session) => [sessionKey(session.hostId, session.id), session],
    ),
  );
  const rows = new Map<string, HostedSession & { updated_at: number }>();
  for (const [hostId, entries] of Object.entries(cache))
    for (const { session, updated_at } of entries) {
      const key = sessionKey(hostId, session.id);
      const value = { ...session, ...live.get(key), hostId, updated_at };
      if (
        !value.archived &&
        value.pinned_at === null &&
        known.has(sessionKey(hostId, value.project_id))
      )
        rows.set(key, value);
    }
  return [...rows.values()].sort(
    (a, b) =>
      b.updated_at - a.updated_at ||
      sessionKey(b.hostId, b.id).localeCompare(sessionKey(a.hostId, a.id)),
  );
}
export function pinnedSessions(
  cache: Record<string, Session[]>,
  projects: HostedProject[],
  hostId: string,
  current: Session[],
): HostedSession[] {
  const knownProjects = new Set(
    projects.map((p) => sessionKey(p.hostId, p.id)),
  );
  const merged = new Map<string, HostedSession>();
  for (const [host, sessions] of Object.entries(cache))
    for (const session of sessions)
      merged.set(sessionKey(host, session.id), { ...session, hostId: host });
  for (const session of current)
    merged.set(sessionKey(hostId, session.id), { ...session, hostId });
  return [...merged.values()]
    .filter(
      (s) =>
        s.pinned_at !== null &&
        !s.archived &&
        knownProjects.has(sessionKey(s.hostId, s.project_id)),
    )
    .sort(
      (a, b) =>
        (b.pinned_at ?? 0) - (a.pinned_at ?? 0) ||
        sessionKey(a.hostId, a.id).localeCompare(sessionKey(b.hostId, b.id)),
    );
}
export function readPinnedCache(): Record<string, Session[]> {
  try {
    const data: unknown = JSON.parse(
      localStorage.getItem("latte-work.pinned-sessions.v1") ?? "{}",
    );
    if (!data || typeof data !== "object" || Array.isArray(data)) return {};
    return Object.fromEntries(
      Object.entries(data)
        .slice(0, 100)
        .filter(
          ([, rows]) =>
            Array.isArray(rows) &&
            rows.length <= 100 &&
            rows.every(
              (s) =>
                s &&
                typeof s.id === "string" &&
                typeof s.project_id === "string" &&
                typeof s.title === "string" &&
                typeof s.pinned_at === "number" &&
                Number.isFinite(s.pinned_at) &&
                typeof s.archived === "boolean" &&
                typeof s.unread === "boolean" &&
                [
                  "ready",
                  "running",
                  "waiting",
                  "completed",
                  "failed",
                  "stopped",
                  "unknown",
                ].includes(s.status),
            ),
        )
        .map(([host, rows]) => [
          host,
          (rows as Session[]).map((session) => ({
            ...session,
            agent_session_open: undefined,
          })),
        ]),
    );
  } catch {
    return {};
  }
}
// Copy only the conversation text; tool payloads may contain large/sensitive raw output.
export function conversationMarkdown(title: string, events: Event[]): string {
  return (
    `# ${title}\n\n` +
    transcript(events)
      .flatMap((item) =>
        (item.type === "user" && !item.hidden) || item.type === "assistant"
          ? [`## ${item.type === "user" ? "用户" : "助手"}\n\n${item.text}`]
          : [],
      )
      .join("\n\n")
  );
}

export async function loadConversation(
  page: (after: number) => Promise<import("./protocol").Response>,
): Promise<Event[]> {
  const result: Event[] = [];
  let cursor = 0;
  let size = 0;
  const started = Date.now();
  for (let batch = 0; batch < 100; batch++) {
    const r = await page(cursor);
    if (r.kind !== "events") throw new Error("无法读取对话记录");
    const next = r.events.at(-1)?.seq ?? cursor;
    if (r.has_more && next <= cursor) throw new Error("对话分页未前进，请重试");
    size += JSON.stringify(r.events).length;
    if (size > 8 * 1024 * 1024 || Date.now() - started > 30_000)
      throw new Error("对话过大或读取超时，未复制不完整内容");
    result.push(...r.events);
    cursor = next;
    if (!r.has_more) return result;
  }
  throw new Error("对话超过 20,000 条事件，未复制不完整内容");
}
