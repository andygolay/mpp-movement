import { useRef, useEffect, useCallback } from "react";
import { useWallet } from "@moveindustries/wallet-adapter-react";
import { useHosts } from "../hooks/useHosts";
import { useCall } from "../hooks/useCall";
import { TOKEN_SYMBOL, TOKEN_DECIMALS } from "../lib/constants";

export function CallerPanel() {
  const { connected } = useWallet();
  const { hosts, loading, error: hostsError } = useHosts();
  const {
    callState,
    duration,
    totalPaid,
    deposit,
    remainingSeconds,
    error: callError,
    remoteStream,
    startCall,
    addTime,
    hangup,
  } = useCall();

  const remoteAudioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    if (remoteAudioRef.current && remoteStream) {
      remoteAudioRef.current.srcObject = remoteStream;
      remoteAudioRef.current.play().catch((e) => {
        console.warn("[caller] audio autoplay blocked:", e.message);
      });
    }
  }, [remoteStream]);

  const formatAmount = useCallback(
    (amount: bigint) => {
      const divisor = BigInt(10 ** TOKEN_DECIMALS);
      const whole = amount / divisor;
      const frac = amount % divisor;
      return `${whole}.${frac.toString().padStart(TOKEN_DECIMALS, "0")}`;
    },
    [],
  );

  function formatDuration(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  function formatRate(rateStr: string): string {
    const rate = BigInt(rateStr);
    const divisor = 10 ** TOKEN_DECIMALS;
    return (Number(rate) / divisor).toFixed(TOKEN_DECIMALS);
  }

  const error = callError || hostsError;

  const availableHosts = hosts.filter((h) => h.online && !h.busy);

  return (
    <div className="panel">
      <h2>Caller Mode</h2>

      {error && <div className="error">{error}</div>}

      {callState === "idle" && (
        <>
          <h3 className="section-title">Available Hosts</h3>
          {loading && <p className="muted">Loading hosts...</p>}
          {!loading && availableHosts.length === 0 && (
            <p className="muted">No hosts are currently live.</p>
          )}
          {availableHosts.map((host) => (
            <div key={host.address} className="host-card">
              <div className="host-info">
                <span className="host-name">{host.name ?? "Host"}</span>
                <span className="host-rate">
                  {formatRate(host.ratePerSecond)} {TOKEN_SYMBOL}/sec
                </span>
              </div>
              <div className="host-address">
                {host.address.slice(0, 8)}...{host.address.slice(-6)}
              </div>
              <button
                className="primary"
                disabled={!connected}
                onClick={() =>
                  startCall(host.address, BigInt(host.ratePerSecond))
                }
              >
                Call
              </button>
            </div>
          ))}
        </>
      )}

      {(callState === "connecting" || callState === "ringing") && (
        <div className="call-status">
          <div className="pulse" />
          <p>
            {callState === "connecting"
              ? "Opening payment channel..."
              : "Ringing..."}
          </p>
          <button className="danger" onClick={hangup}>
            Cancel
          </button>
        </div>
      )}

      {callState === "in_call" && (
        <div className="call-active">
          <div className="call-timer">{formatDuration(duration)}</div>
          <div className="status-grid">
            <div className="status-item">
              <label>Total Paid</label>
              <div className="value green">
                {formatAmount(totalPaid)} {TOKEN_SYMBOL}
              </div>
            </div>
            <div className="status-item">
              <label>Remaining</label>
              <div
                className="value"
                style={{
                  color:
                    remainingSeconds <= 30
                      ? "var(--red, #ff4444)"
                      : remainingSeconds <= 60
                        ? "var(--yellow, #ffaa00)"
                        : "var(--blue, #4488ff)",
                }}
              >
                {formatDuration(remainingSeconds)}
              </div>
            </div>
          </div>
          {remainingSeconds <= 60 && (
            <div
              style={{
                padding: "0.5rem 0.75rem",
                borderRadius: "0.5rem",
                background:
                  remainingSeconds <= 30
                    ? "rgba(255,68,68,0.15)"
                    : "rgba(255,170,0,0.15)",
                color:
                  remainingSeconds <= 30
                    ? "var(--red, #ff4444)"
                    : "var(--yellow, #ffaa00)",
                fontSize: "0.85rem",
                marginTop: "0.5rem",
                textAlign: "center",
              }}
            >
              {remainingSeconds <= 30
                ? "Running out of time! Add more to keep the call going."
                : "Time is getting low."}
            </div>
          )}
          <div style={{ display: "flex", gap: "0.5rem", marginTop: "1rem" }}>
            <button
              className="primary"
              onClick={() => addTime(300)}
              style={{ flex: 1 }}
            >
              +5 min
            </button>
            <button
              className="danger"
              onClick={hangup}
              style={{ flex: 1 }}
            >
              Hang Up
            </button>
          </div>
        </div>
      )}

      {callState === "hanging_up" && (
        <div className="call-status">
          <p>Ending call...</p>
        </div>
      )}

      {callState === "ended" && (
        <div className="call-summary">
          <h3>Call Ended</h3>
          <div className="status-grid">
            <div className="status-item">
              <label>Duration</label>
              <div className="value">{formatDuration(duration)}</div>
            </div>
            <div className="status-item">
              <label>Total Cost</label>
              <div className="value green">
                {formatAmount(totalPaid)} {TOKEN_SYMBOL}
              </div>
            </div>
          </div>
          <button
            onClick={() => window.location.reload()}
            style={{ marginTop: "1rem" }}
          >
            New Call
          </button>
        </div>
      )}

      {/* Hidden audio element for remote audio playback */}
      <audio ref={remoteAudioRef} autoPlay style={{ display: "none" }} />
    </div>
  );
}
