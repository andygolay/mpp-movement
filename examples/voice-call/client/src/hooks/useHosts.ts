import { useEffect, useState } from "react";
import { SERVER_URL } from "../lib/constants";

export interface HostInfo {
  address: string;
  name: string | null;
  ratePerSecond: string;
  currency: string;
  online: boolean;
  busy: boolean;
}

/**
 * Polls GET /api/hosts every 3 seconds and returns the list.
 */
export function useHosts() {
  const [hosts, setHosts] = useState<HostInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      try {
        const resp = await fetch(`${SERVER_URL}/api/hosts`);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const data = await resp.json();
        if (!cancelled) {
          setHosts(Array.isArray(data) ? data : []);
          setError("");
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    poll();
    const interval = setInterval(poll, 3000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  return { hosts, loading, error };
}
