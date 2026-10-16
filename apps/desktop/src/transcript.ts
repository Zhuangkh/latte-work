import type { Event, TurnChanges, QuestionAnswer } from "./protocol";
/** Undo/status events for older turns must not replace the latest turn. */
export function latestTurnChanges(events: Event[]): TurnChanges | undefined {
  type RecordedTurn = { changes: TurnChanges; at: number; seq: number };
  const turns = new Map<string, RecordedTurn>();
  for (const { event, at, seq } of events) {
    if (event.kind !== "turn_changes") continue;
    const changes = event.changes;
    const previous = turns.get(changes.request_id);
    turns.set(changes.request_id, {
      changes,
      at: changes.summary.baseline_at ?? previous?.at ?? at,
      seq: previous?.seq ?? seq,
    });
  }
  return [...turns.values()].reduce<RecordedTurn | undefined>(
    (latest, turn) =>
      !latest ||
      turn.at > latest.at ||
      (turn.at === latest.at && turn.seq > latest.seq)
        ? turn
        : latest,
    undefined,
  )?.changes;
}
export type Item =
  | {
      key: number;
      type: "user";
      text: string;
      at: number;
      handoff?: true;
      hidden?: true;
    }
  | { key: number; type: "assistant"; text: string }
  | {
      key: number;
      type: "event";
      value: Event["event"];
      resolved?: boolean;
      answers?: QuestionAnswer[] | null;
      decision?: "allowed" | "denied" | "expired";
    };
export function appendEvents(existing: Event[], incoming: Event[]): Event[] {
  const seen = new Set(existing.map((e) => e.seq));
  return [...existing, ...incoming.filter((e) => !seen.has(e.seq))].sort(
    (a, b) => a.seq - b.seq,
  );
}
const inputKey = (sessionId: string, requestId: string) =>
  JSON.stringify([sessionId, requestId]);

/** Correlate delivery and execution without matching unrelated repeated text. */
export function queuedInputReceipts(events: Event[]) {
  const receipts = new Map<string, { text: string; seq: number }>();
  for (const { event, session_id, seq } of events) {
    if (event.kind !== "queued_message_sent") continue;
    const key = inputKey(session_id, event.request_id);
    if (!receipts.has(key)) receipts.set(key, { text: event.text, seq });
  }
  return receipts;
}

export function transcript(
  events: Event[],
  receipts = queuedInputReceipts(events),
): Item[] {
  const items: Item[] = [];
  const resolved = new Map(
    events.flatMap((e) =>
      e.event.kind === "approval_resolved"
        ? [[e.event.request_id, e.event.allow] as const]
        : [],
    ),
  );
  const answered = new Map(
    events.flatMap(({ event }) =>
      event.kind === "question_resolved"
        ? [[event.request_id, event.answers] as const]
        : [],
    ),
  );
  const terminal = events
    .filter(
      (e) =>
        e.event.kind === "state" &&
        !["running", "waiting"].includes(e.event.status),
    )
    .map((e) => e.seq);
  const turnChanges = new Map(
    events.flatMap(({ event }) =>
      event.kind === "turn_changes"
        ? [[event.changes.request_id, event] as const]
        : [],
    ),
  );
  const renderedChanges = new Set<string>();
  for (const { seq, event, at, session_id } of events) {
    if (event.kind === "turn_changes") {
      if (!renderedChanges.has(event.changes.request_id)) {
        items.push({
          key: seq,
          type: "event",
          value: turnChanges.get(event.changes.request_id)!,
        });
        renderedChanges.add(event.changes.request_id);
      }
      continue;
    }
    if (
      event.kind === "usage" ||
      event.kind === "progress" ||
      event.kind === "subagent"
    )
      continue;
    if (event.kind === "queued_message_sent") {
      if (receipts.get(inputKey(session_id, event.request_id))?.seq === seq)
        items.push({
          key: seq,
          type: "user",
          text: event.text,
          at,
          handoff: true,
        });
    } else if (event.kind === "user") {
      const receipt = receipts.get(inputKey(session_id, event.request_id));
      items.push({
        key: seq,
        type: "user",
        text: event.text,
        at,
        ...(receipt?.text === event.text && receipt.seq < seq
          ? { hidden: true as const }
          : {}),
      });
    } else if (event.kind === "text") {
      const last = items.at(-1);
      if (last?.type === "assistant") last.text += event.text;
      else items.push({ key: seq, type: "assistant", text: event.text });
    } else if (event.kind === "question") {
      items.push({
        key: seq,
        type: "event",
        value: event,
        resolved:
          answered.has(event.request_id) || terminal.some((end) => end > seq),
        answers: answered.get(event.request_id),
      });
    } else if (event.kind === "approval") {
      const inactive = terminal.some((end) => end > seq);
      items.push({
        key: seq,
        type: "event",
        value: event,
        resolved: inactive || resolved.has(event.request_id),
        decision: resolved.has(event.request_id)
          ? resolved.get(event.request_id)
            ? "allowed"
            : "denied"
          : inactive
            ? "expired"
            : undefined,
      });
    } else if (
      event.kind === "state"
        ? !!event.message
        : event.kind !== "approval_resolved" &&
          event.kind !== "question_resolved"
    )
      items.push({ key: seq, type: "event", value: event });
  }
  return items;
}
