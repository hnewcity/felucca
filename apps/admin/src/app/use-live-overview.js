import { useEffect, useState } from "react";
import { useAdmin } from "./context.jsx";

// Runtime status updates independently of editable configuration snapshots.
// Polling must not reset an open form or silently advance its write revision.
export function useLiveOverview(initial) {
  const { api } = useAdmin();
  const [overview, setOverview] = useState(initial);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true,
      timer;
    const abort = new AbortController();
    async function poll() {
      try {
        const result = await api("/api/overview", "GET", undefined, { signal: abort.signal });
        if (active) {
          setOverview(result);
          setError("");
        }
      } catch (error) {
        if (active && error.name !== "AbortError") setError(error.message);
      }
      if (active) timer = setTimeout(poll, 3000);
    }
    timer = setTimeout(poll, 3000);
    return () => {
      active = false;
      clearTimeout(timer);
      abort.abort();
    };
  }, [api]);
  return { overview, error };
}
