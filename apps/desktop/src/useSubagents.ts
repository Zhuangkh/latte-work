import { useCallback, useState } from "react";
import { request } from "./api";
import { LIVE_POLL_MS, usePollingSnapshot } from "./usePollingSnapshot";
import type { Event, Subagent } from "./protocol";
const empty = { tasks: [] as Subagent[], truncated: false };

export interface SubagentsState {
  connectionVersion?: number;
  tasks: Subagent[];
  loading: boolean;
  error: string;
  truncated: boolean;
  reload?: () => void;
}
export function useSubagents(
  hostId: string,
  sessionId: string | undefined,
  connected: boolean,
  events: Event[],
  connectionVersion = 0,
  visible = false,
): SubagentsState {
  const [reloadVersion, setReloadVersion] = useState(0);
  const reload = () => setReloadVersion((value) => value + 1);
  const scope = JSON.stringify([hostId, sessionId]);
  const revision = events
    .filter((e) => e.session_id === sessionId && e.event.kind === "subagent")
    .at(-1)?.seq;
  const read = useCallback(async () => {
    const response = await request(hostId, {
      method: "subagents",
      session_id: sessionId!,
    });
    if (response.kind !== "subagents")
      throw new Error("此 Server 不支持子智能体列表，请更新 Server");
    return { tasks: response.tasks, truncated: response.truncated };
  }, [hostId, sessionId]);
  const snapshot = usePollingSnapshot(
    scope,
    !!sessionId && connected,
    read,
    empty,
    JSON.stringify([revision, reloadVersion]),
    visible ? LIVE_POLL_MS : 0,
    connectionVersion,
  );
  const state = {
    ...snapshot.data,
    loading: snapshot.loading,
    error: snapshot.error,
  };
  return {
    ...state,
    connectionVersion,
    reload,
    tasks:
      connected && snapshot.confirmed
        ? state.tasks
        : state.tasks.map((task) =>
            ["running", "paused"].includes(task.status)
              ? { ...task, status: "unknown" as const }
              : task,
          ),
  };
}
