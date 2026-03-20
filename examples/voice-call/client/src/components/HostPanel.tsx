import { useCallback, useEffect, useRef, useState } from "react";
import { useWallet } from "@moveindustries/wallet-adapter-react";
import { verifyVoucher, hexToBytes, toHex } from "@mpp/client";
import { SERVER_URL, MODULE_ADDRESS, REGISTRY_ADDR, TOKEN_SYMBOL, TOKEN_DECIMALS } from "../lib/constants";
import { createPeerConnection, getUserAudio } from "../lib/webrtc";
import { connectSignaling, type SignalingConnection } from "../lib/signaling";

/** Decode a hex string (with optional 0x prefix) to raw bytes without padding. */
function rawHexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

type HostStatus = "offline" | "online" | "in_call" | "settling";

// Keep this unused in the type but used as a ref value
// to guard against concurrent endCall/handleHangup.

export function HostPanel() {
  const { account, connected, signAndSubmitTransaction } = useWallet();

  const [displayName, setDisplayName] = useState("Host");
  const [rateInput, setRateInput] = useState("0.001");
  const [status, setStatus] = useState<HostStatus>("offline");
  const [callDuration, setCallDuration] = useState(0);
  const [callerAddress, setCallerAddress] = useState("");
  const [earnings, setEarnings] = useState(0n);
  const [error, setError] = useState("");
  const callIdRef = useRef<string>("");

  const signalingRef = useRef<SignalingConnection | null>(null);
  const peerConnectionRef = useRef<RTCPeerConnection | null>(null);
  const localStreamRef = useRef<MediaStream | null>(null);
  const remoteAudioRef = useRef<HTMLAudioElement | null>(null);
  const durationIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const statusRef = useRef<HostStatus>("offline");

  // Voucher tracking (received via WebRTC data channel)
  const highestVoucherRef = useRef<{
    channelId: string;
    cumulativeAmount: string;
    signature: string;
    pubkey: string;
  } | null>(null);

  const rateOctas = BigInt(
    Math.round(parseFloat(rateInput || "0") * 10 ** TOKEN_DECIMALS),
  );

  const formatAmount = useCallback(
    (amount: bigint) => {
      const divisor = 10 ** TOKEN_DECIMALS;
      return (Number(amount) / divisor).toFixed(TOKEN_DECIMALS);
    },
    [],
  );

  const cleanup = useCallback(() => {
    if (durationIntervalRef.current) {
      clearInterval(durationIntervalRef.current);
      durationIntervalRef.current = null;
    }
    if (peerConnectionRef.current) {
      peerConnectionRef.current.close();
      peerConnectionRef.current = null;
    }
    if (localStreamRef.current) {
      localStreamRef.current.getTracks().forEach((t) => t.stop());
      localStreamRef.current = null;
    }
    if (signalingRef.current) {
      signalingRef.current.close();
      signalingRef.current = null;
    }
  }, []);

  // Poll for incoming calls when online
  useEffect(() => {
    if (status !== "online" || !account) return;

    const poll = async () => {
      try {
        const resp = await fetch(
          `${SERVER_URL}/api/host/poll?address=${account.address}`,
        );
        if (!resp.ok) return;
        const data = await resp.json();
        if (data.callId && statusRef.current === "online") {
          handleIncomingCall(data.callId, data.callerAddress ?? "");
        }
      } catch {
        // ignore polling errors
      }
    };

    pollIntervalRef.current = setInterval(poll, 2000);
    poll();

    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current);
        pollIntervalRef.current = null;
      }
    };
  }, [status, account]);

  async function handleGoLive() {
    if (!account) return;
    setError("");

    try {
      const resp = await fetch(`${SERVER_URL}/api/host/go-live`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          address: account.address.toString(),
          ratePerSecond: rateOctas.toString(),
          currency: "0xa",
          name: displayName,
        }),
      });

      if (!resp.ok) {
        const body = await resp.text();
        throw new Error(`Registration failed: ${body}`);
      }

      setStatus("online");
      statusRef.current = "online";
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleGoOffline() {
    cleanup();
    if (pollIntervalRef.current) {
      clearInterval(pollIntervalRef.current);
      pollIntervalRef.current = null;
    }

    try {
      await fetch(`${SERVER_URL}/api/host/go-live`, {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          address: account?.address?.toString(),
        }),
      });
    } catch {
      // best-effort
    }

    setStatus("offline");
    statusRef.current = "offline";
    setCallDuration(0);
    setCallerAddress("");
    setEarnings(0n);
  }

  async function handleIncomingCall(callId: string, caller: string) {
    setStatus("in_call");
    statusRef.current = "in_call";
    callIdRef.current = callId;
    highestVoucherRef.current = null;
    setCallerAddress(caller);
    setCallDuration(0);
    setEarnings(0n);

    try {
      // Get microphone
      const localStream = await getUserAudio();
      localStreamRef.current = localStream;

      // Create peer connection
      const pc = createPeerConnection((stream) => {
        // Play remote audio
        if (remoteAudioRef.current) {
          remoteAudioRef.current.srcObject = stream;
          remoteAudioRef.current.play().catch(() => {});
        }
      });
      peerConnectionRef.current = pc;

      // Listen for the "vouchers" data channel from the caller
      pc.ondatachannel = (event) => {
        const dc = event.channel;
        if (dc.label !== "vouchers") return;

        dc.onmessage = (msgEvent) => {
          try {
            const data = JSON.parse(msgEvent.data as string) as {
              channelId: string;
              cumulativeAmount: string;
              signature: string;
              pubkey: string;
            };

            const cumulative = BigInt(data.cumulativeAmount);
            const currentHighest = highestVoucherRef.current;

            // Only accept if cumulative amount increases
            if (currentHighest && cumulative <= BigInt(currentHighest.cumulativeAmount)) {
              return;
            }

            // Verify the voucher signature
            const channelIdBytes = hexToBytes(data.channelId);
            const signatureBytes = rawHexToBytes(data.signature);
            const pubkeyBytes = hexToBytes(data.pubkey);

            const valid = verifyVoucher(
              { channelId: channelIdBytes, cumulativeAmount: cumulative },
              signatureBytes,
              pubkeyBytes,
            );

            if (!valid) {
              console.warn("Invalid voucher signature received");
              return;
            }

            // Store the highest valid voucher
            highestVoucherRef.current = data;
            setEarnings(cumulative);
          } catch (e) {
            console.warn("Failed to parse voucher message:", e);
          }
        };
      };

      // Add local audio tracks
      localStream.getTracks().forEach((track) => {
        pc.addTrack(track, localStream);
      });

      // Connect signaling
      const wsUrl = `${SERVER_URL.replace(/^http/, "ws")}/ws/signal/${callId}?address=${account?.address}`;
      const signaling = connectSignaling(wsUrl, {
        onOffer: async (offer) => {
          console.log("[host] received offer");
          await pc.setRemoteDescription(new RTCSessionDescription(offer));
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          console.log("[host] sending answer");
          signaling.send({
            type: "answer",
            answer: pc.localDescription!.toJSON(),
          });
        },
        onIceCandidate(candidate) {
          pc.addIceCandidate(new RTCIceCandidate(candidate));
        },
        onError(msg) {
          setError(`Signaling: ${msg}`);
        },
        onClose() {
          if (statusRef.current === "in_call") {
            endCall();
          }
        },
      });
      signalingRef.current = signaling;

      // Send ICE candidates
      pc.onicecandidate = (event) => {
        if (event.candidate) {
          signaling.send({
            type: "ice-candidate",
            candidate: event.candidate.toJSON(),
          });
        }
      };

      // Duration timer (earnings are updated from voucher data channel)
      const startTime = Date.now();
      durationIntervalRef.current = setInterval(() => {
        const elapsed = Math.floor((Date.now() - startTime) / 1000);
        setCallDuration(elapsed);
      }, 1000);

      pc.onconnectionstatechange = () => {
        if (
          pc.connectionState === "disconnected" ||
          pc.connectionState === "failed"
        ) {
          if (statusRef.current === "in_call") {
            endCall();
          }
        }
      };
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      endCall();
    }
  }

  function endCall() {
    // Called by WebSocket/peer disconnect — ignore if already hanging up.
    if (statusRef.current !== "in_call") return;
    // Don't close channel here — just clean up the connection.
    // The host still has the voucher data and can close manually.
    cleanup();
    if (callIdRef.current) {
      fetch(`${SERVER_URL}/api/call/hangup`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ callId: callIdRef.current }),
      }).catch(() => {});
    }
    callIdRef.current = "";
    setStatus("online");
    statusRef.current = "online";
    setCallerAddress("");
    setCallDuration(0);
  }

  async function handleHangup() {
    // Prevent endCall from racing with us.
    statusRef.current = "settling";

    // Close the channel on-chain FIRST (before cleanup kills the connection).
    const voucher = highestVoucherRef.current;
    if (voucher && voucher.cumulativeAmount !== "0") {
      try {
        const channelIdArray = Array.from(rawHexToBytes(voucher.channelId));
        const sigArray = Array.from(rawHexToBytes(voucher.signature));
        const pubkeyArray = Array.from(rawHexToBytes(voucher.pubkey));

        await signAndSubmitTransaction({
          data: {
            function: `${MODULE_ADDRESS}::channel::close` as `${string}::${string}::${string}`,
            functionArguments: [
              REGISTRY_ADDR,
              channelIdArray,
              Number(voucher.cumulativeAmount),
              sigArray,
              pubkeyArray,
            ],
          },
        } as never);
      } catch (e) {
        setError(`Close failed: ${e instanceof Error ? e.message : String(e)}`);
      }
    }

    // Now clean up the call.
    cleanup();
    try {
      if (callIdRef.current) {
        await fetch(`${SERVER_URL}/api/call/hangup`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ callId: callIdRef.current }),
        });
      }
    } catch {
      // best effort
    }

    highestVoucherRef.current = null;
    callIdRef.current = "";
    setStatus("online");
    statusRef.current = "online";
    setCallerAddress("");
    setCallDuration(0);
    setEarnings(0n);
  }

  const statusLabel =
    status === "offline"
      ? "Offline"
      : status === "online"
        ? "Live - Waiting for calls"
        : "In Call";

  const statusColor =
    status === "offline"
      ? "var(--text-muted)"
      : status === "online"
        ? "var(--green)"
        : "var(--yellow)";

  function formatDuration(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  return (
    <div className="panel">
      <h2>Host Mode</h2>

      <div className="status-badge" style={{ color: statusColor }}>
        {statusLabel}
      </div>

      {error && <div className="error">{error}</div>}

      {status === "offline" && (
        <>
          <div className="field-group">
            <label>Display Name</label>
            <input
              type="text"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Your name"
            />
          </div>
          <div className="field-group">
            <label>Rate per second ({TOKEN_SYMBOL})</label>
            <input
              type="text"
              value={rateInput}
              onChange={(e) => setRateInput(e.target.value)}
              placeholder="0.001"
            />
          </div>
          <button
            className="primary"
            disabled={!connected || !displayName.trim() || rateOctas <= 0n}
            onClick={handleGoLive}
          >
            Go Live
          </button>
        </>
      )}

      {status === "online" && (
        <button className="danger" onClick={handleGoOffline}>
          Go Offline
        </button>
      )}

      {status === "in_call" && (
        <div className="call-info">
          <div className="status-grid">
            <div className="status-item">
              <label>Caller</label>
              <div className="value" style={{ fontSize: "0.75rem" }}>
                {callerAddress
                  ? `${callerAddress.slice(0, 8)}...${callerAddress.slice(-6)}`
                  : "Unknown"}
              </div>
            </div>
            <div className="status-item">
              <label>Duration</label>
              <div className="value blue">{formatDuration(callDuration)}</div>
            </div>
            <div className="status-item">
              <label>Earnings</label>
              <div className="value green">
                {formatAmount(earnings)} {TOKEN_SYMBOL}
              </div>
            </div>
          </div>
          <button
            className="danger"
            onClick={handleHangup}
            style={{ marginTop: "1rem" }}
          >
            Hang Up
          </button>
        </div>
      )}

      {/* Hidden audio element for remote audio playback */}
      <audio ref={remoteAudioRef} autoPlay style={{ display: "none" }} />
    </div>
  );
}
