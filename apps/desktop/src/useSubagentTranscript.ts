import { useCallback, useEffect, useMemo, useState } from "react";
import { message, request } from "./api";
import type { Event } from "./protocol";
import { LIVE_POLL_MS, usePollingSnapshot } from "./usePollingSnapshot";
const empty = {
  events: [] as Event[],
  prompt: null as string | null,
  incomplete: false,
  readingMore: false,
  page: 0,
};

export function useSubagentTranscript(
  hostId: string,
  sessionId: string | undefined,
  taskId: string | undefined,
  active: boolean,
  running: boolean,
  revision?: number,
  connectionVersion = 0,
) {
  const scope = JSON.stringify([hostId, sessionId, taskId]);
  const [reloadVersion, setReloadVersion] = useState(0);
  // A scope owns its cursor and cache. Late reads cannot mutate a new child's
  // cursor, even when its sequence numbers happen to match.
  const cache = useMemo(
    () => ({
      events: new Map<number, Event>(),
      after: null as number | null,
      source: null as string | null,
      alive: true,
      page: 0,
    }),
    [scope, connectionVersion],
  );
  const enabled = active && !!sessionId && !!taskId;
  useEffect(() => {
    cache.alive = enabled;
    return () => {
      cache.alive = false;
    };
  }, [cache, enabled]);
  const read = useCallback(async () => {
    try {
      const after = cache.after;
      const response = await request(hostId, {
        method: "subagent_history",
        session_id: sessionId!,
        task_id: taskId!,
        after,
        source: cache.source,
      });
      if (response.kind !== "subagent_history")
        throw new Error("此 Server 不支持完整子智能体记录，请更新 Server");
      if (!cache.alive) return empty;
      if (response.source !== cache.source) cache.events.clear();
      if (
        response.has_more &&
        (response.after === null ||
          (response.after === after && response.source === cache.source))
      )
        throw new Error("子智能体记录分页未前进，请重试");
      cache.source = response.source;
      cache.after = response.after;
      for (const event of response.events) cache.events.set(event.seq, event);
      return {
        events: [...cache.events.values()].sort((a, b) => a.seq - b.seq),
        prompt: response.prompt ?? null,
        incomplete: response.incomplete,
        readingMore: response.has_more || response.source_has_more,
        page: ++cache.page,
      };
    } catch (cause) {
      if (message(cause).includes("unknown variant `subagent_history`"))
        throw new Error("此 Server 不支持完整子智能体记录，请更新 Server");
      throw cause;
    }
  }, [hostId, sessionId, taskId, cache]);
  const state = usePollingSnapshot(
    scope,
    enabled,
    read,
    empty,
    JSON.stringify([revision, running, reloadVersion]),
    running ? LIVE_POLL_MS : 0,
    connectionVersion,
  );
  // Continue bounded pages while visible, including after task completion.
  // The snapshot hook serializes refreshes and fences unmounted/old connections.
  useEffect(() => {
    if (enabled && state.data.readingMore && !state.error)
      setReloadVersion((v) => v + 1);
  }, [enabled, state.data.page, state.data.readingMore, state.error]);
  return {
    ...state.data,
    loading: state.loading,
    error: state.error,
    reload: () => setReloadVersion((v) => v + 1),
  };
}
