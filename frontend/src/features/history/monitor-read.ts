// One read loop shared by the monitoring views. A refresh keeps the last
// result on screen and only marks it stale when the read fails; a change of
// scope (target, window, selection) drops it, so one target's numbers never
// sit under another target's name.
import { useCallback, useEffect, useRef, useState } from "react";

export function failureId(cause: unknown, fallback: string): string {
  return typeof cause === "object" && cause !== null && "message_id" in cause && typeof cause.message_id === "string" ? cause.message_id : fallback;
}

export type MonitorRead<T> = {
  data: T | null;
  /** The last read failed; `data`, when present, is from `updatedSeconds`. */
  error: string | null;
  updatedSeconds: number | null;
  loading: boolean;
  refresh(): void;
};

export type MonitorState<T> = { scope: string; data: T | null; error: string | null; updatedSeconds: number | null; loading: boolean };

/** What a view may show for `scope`: nothing that was read for another scope. */
export function monitorView<T>(state: MonitorState<T>, scope: string, enabled: boolean): Omit<MonitorRead<T>, "refresh"> {
  if (state.scope !== scope) return { data: null, error: null, updatedSeconds: null, loading: enabled };
  return { data: state.data, error: state.error, updatedSeconds: state.updatedSeconds, loading: state.loading };
}

/**
 * Reads when `scope` or `generation` changes or on `refresh()`. `generation`
 * is the collector's progress, so views follow new data without polling
 * StarRocks or the local API on their own timer. `read` null means disabled.
 */
export function useMonitorRead<T>(read: ((signal: AbortSignal) => Promise<T>) | null, scope: string, generation: string, fallback: string): MonitorRead<T> {
  const [state, setState] = useState<MonitorState<T>>({ scope, data: null, error: null, updatedSeconds: null, loading: read !== null });
  const [tick, setTick] = useState(0);
  const refresh = useCallback(() => setTick((value) => value + 1), []);
  const readRef = useRef(read);
  useEffect(() => { readRef.current = read; });
  const enabled = read !== null;
  useEffect(() => {
    const current = readRef.current;
    if (!current) { setState({ scope, data: null, error: null, updatedSeconds: null, loading: false }); return; }
    const controller = new AbortController();
    setState((previous) => previous.scope === scope ? { ...previous, loading: true } : { scope, data: null, error: null, updatedSeconds: null, loading: true });
    current(controller.signal)
      .then((value) => { if (!controller.signal.aborted) setState({ scope, data: value, error: null, updatedSeconds: Math.floor(Date.now() / 1000), loading: false }); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setState((previous) => ({ ...previous, scope, error: failureId(cause, fallback), loading: false })); });
    return () => controller.abort();
  }, [scope, generation, tick, enabled, fallback]);
  return { ...monitorView(state, scope, enabled), refresh };
}
