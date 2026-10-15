import { useEffect, useRef, useState } from "react";
import { message } from "./api";

export const LIVE_POLL_MS = 650;

/** One read at a time. Metadata changes queue a refresh without discarding that read. */
export function usePollingSnapshot<T>(
  scope: string,
  enabled: boolean,
  read: () => Promise<T>,
  initial: T,
  signal: string,
  interval: number,
  connectionVersion: number,
) {
  const [state, setState] = useState({
    scope,
    data: initial,
    loading: false,
    error: "",
    capturedVersion: -1,
  });
  const refresh = useRef<((key: string) => void) | undefined>(undefined);
  const reschedule = useRef<((period: number) => void) | undefined>(undefined);
  const latestInterval = useRef(interval);
  latestInterval.current = interval;
  const latestSignal = useRef(signal);
  latestSignal.current = signal;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let pending = false;
    let queued = false;
    let lastSignal = latestSignal.current;
    let lastInterval = latestInterval.current;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = async () => {
      clearTimeout(timer);
      if (disposed) return;
      if (pending) {
        queued = true;
        return;
      }
      pending = true;
      setState((old) =>
        old.scope === scope
          ? {
              ...old,
              loading: old.capturedVersion !== connectionVersion || !!old.error,
            }
          : {
              scope,
              data: initial,
              loading: true,
              error: "",
              capturedVersion: -1,
            },
      );
      try {
        const data = await read();
        if (!disposed)
          setState({
            scope,
            data,
            loading: false,
            error: "",
            capturedVersion: connectionVersion,
          });
      } catch (cause) {
        if (!disposed)
          setState((old) => ({
            ...old,
            scope,
            loading: false,
            error: message(cause),
          }));
      } finally {
        pending = false;
        if (!disposed) {
          if (queued) {
            queued = false;
            void load();
          } else if (latestInterval.current > 0)
            timer = setTimeout(() => void load(), latestInterval.current);
        }
      }
    };
    refresh.current = (key) => {
      if (key === lastSignal) return;
      lastSignal = key;
      void load();
    };
    reschedule.current = (period) => {
      if (period === lastInterval) return;
      lastInterval = period;
      clearTimeout(timer);
      if (period > 0) void load();
    };
    void load();
    return () => {
      disposed = true;
      clearTimeout(timer);
      refresh.current = undefined;
      reschedule.current = undefined;
    };
  }, [scope, enabled, read, connectionVersion]);

  useEffect(() => {
    refresh.current?.(signal);
  }, [signal]);
  useEffect(() => {
    reschedule.current?.(interval);
  }, [interval]);
  const value =
    state.scope === scope
      ? state
      : {
          scope,
          data: initial,
          loading: enabled,
          error: "",
          capturedVersion: -1,
        };
  return { ...value, confirmed: value.capturedVersion === connectionVersion };
}
