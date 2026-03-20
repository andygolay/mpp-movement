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
    error: callError,
    remoteStream,
    startCall,
    hangup,
  } = useCall();

  const remoteAudioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    if (remoteAudioRef.current && remoteStream) {
      remoteAudioRef.current.srcObject = remoteStream;
      remoteAudioRef.current.play().catch(() => {});
    }
  }, [remoteStream]);

  const formatAmount = useCallback(
    (amount: bigint) => {
      const divisor = 10 ** TOKEN_DECIMALS;
      return (Number(amount) / divisor).toFixed(TOKEN_DECIMALS);
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
              <label>Duration</label>
              <div className="value blue">{formatDuration(duration)}</div>
            </div>
          </div>
          <button
            className="danger"
            onClick={hangup}
            style={{ marginTop: "1rem", width: "100%" }}
          >
            Hang Up
          </button>
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
