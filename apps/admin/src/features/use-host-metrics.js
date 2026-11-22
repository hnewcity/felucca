import { useCallback, useEffect, useState } from "react";

export function useHostMetrics(api, windowSeconds) {
  const [snapshot, setSnapshot] = useState(null);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [version, setVersion] = useState(0);
  const refresh = useCallback(() => setVersion((value) => value + 1), []);

  useEffect(() => {
    let active = true;
    let timer;
    let inFlight = false;
    const controller = new AbortController();
    async function read() {
      clearTimeout(timer);
      if (inFlight || !active) return;
      if (document.hidden) {
        timer = setTimeout(read, 5000);
        return;
      }
      inFlight = true;
      setRefreshing(true);
      try {
        const next = await api(`/api/monitor?window=${windowSeconds}`, "GET", undefined, {
          signal: AbortSignal.any([controller.signal, AbortSignal.timeout(12000)]),
        });
        if (active) {
          setSnapshot({ ...next, windowSeconds });
          setError("");
        }
      } catch (err) {
        if (active && err.name !== "AbortError") setError(err.message);
      } finally {
        inFlight = false;
        if (active) {
          setRefreshing(false);
          timer = setTimeout(read, 5000);
        }
      }
    }
    const visible = () => {
      if (!document.hidden) read();
    };
    read();
    document.addEventListener("visibilitychange", visible);
    return () => {
      active = false;
      clearTimeout(timer);
      controller.abort();
      document.removeEventListener("visibilitychange", visible);
    };
  }, [api, windowSeconds, version]);

  return { snapshot, error, refreshing, refresh };
}
