import { useCallback, useRef, useState } from "react";
import { useWallet } from "@moveindustries/wallet-adapter-react";
import {
  MovementSessionProvider,
  parseWwwAuthenticate,
  formatAuthorization,
  toHex,
  type SessionProviderOptions,
} from "@mpp/client";
import {
  SERVER_URL,
  MODULE_ADDRESS,
  TOKEN_METADATA_ADDR,
  TOKEN_DECIMALS,
} from "../lib/constants";
import { createPeerConnection, getUserAudio } from "../lib/webrtc";
import { connectSignaling, type SignalingConnection } from "../lib/signaling";

export type CallState =
  | "idle"
  | "connecting"
  | "ringing"
  | "in_call"
  | "hanging_up"
  | "ended";

interface CallResult {
  callState: CallState;
  duration: number;
  totalPaid: bigint;
  error: string;
  remoteStream: MediaStream | null;
  startCall: (hostAddress: string, ratePerSecond: bigint) => Promise<void>;
  hangup: () => Promise<void>;
}

/**
 * Manages the full call lifecycle:
 * 1. POST /api/call/start -> 402 -> open channel via wallet -> get callId
 * 2. Connect WebSocket signaling
 * 3. Create RTCPeerConnection, exchange SDP
 * 4. Start voucher loop (every 5s, send voucher over WebRTC data channel)
 * 5. On hangup: stop loop, close peer connection, POST /api/call/hangup
 */
export function useCall(): CallResult {
  const { account, signAndSubmitTransaction } = useWallet();

  const [callState, setCallState] = useState<CallState>("idle");
  const [duration, setDuration] = useState(0);
  const [totalPaid, setTotalPaid] = useState(0n);
  const [error, setError] = useState("");
  const [remoteStream, setRemoteStream] = useState<MediaStream | null>(null);

  const sessionProviderRef = useRef<MovementSessionProvider | null>(null);
  const signalingRef = useRef<SignalingConnection | null>(null);
  const peerConnectionRef = useRef<RTCPeerConnection | null>(null);
  const dataChannelRef = useRef<RTCDataChannel | null>(null);
  const localStreamRef = useRef<MediaStream | null>(null);
  const voucherIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const durationIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const callIdRef = useRef<string>("");
  const hostAddressRef = useRef<string>("");
  const rateRef = useRef<bigint>(0n);
  const callStateRef = useRef<CallState>("idle");

  const cleanup = useCallback(() => {
    if (voucherIntervalRef.current) {
      clearInterval(voucherIntervalRef.current);
      voucherIntervalRef.current = null;
    }
    if (durationIntervalRef.current) {
      clearInterval(durationIntervalRef.current);
      durationIntervalRef.current = null;
    }
    if (dataChannelRef.current) {
      dataChannelRef.current.close();
      dataChannelRef.current = null;
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

  const startCall = useCallback(
    async (hostAddress: string, ratePerSecond: bigint) => {
      if (!account) {
        setError("Wallet not connected");
        return;
      }

      if (account.address.toString().toLowerCase() === hostAddress.toLowerCase()) {
        setError("Can't call yourself — use a different wallet for the caller");
        return;
      }

      setError("");
      setDuration(0);
      setTotalPaid(0n);
      setRemoteStream(null);
      setCallState("connecting");
      callStateRef.current = "connecting";
      hostAddressRef.current = hostAddress;
      rateRef.current = ratePerSecond;

      try {
        // 1. Hit /api/call/start -> expect 402
        const startResp = await fetch(
          `${SERVER_URL}/api/call/start?host=${hostAddress}`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ address: account.address.toString() }),
          },
        );

        if (startResp.status !== 402) {
          const body = await startResp.text();
          throw new Error(
            `Expected 402, got ${startResp.status}: ${body}`,
          );
        }

        // Parse the 402 challenge
        const wwwAuth = startResp.headers.get("www-authenticate");
        if (!wwwAuth) throw new Error("No WWW-Authenticate header in 402");
        const challenge = parseWwwAuthenticate(wwwAuth);

        // 2. Create session provider and pay (opens channel on-chain)
        const providerOpts: SessionProviderOptions = {
          moduleAddress: MODULE_ADDRESS,
          tokenMetadata: TOKEN_METADATA_ADDR,
        };

        const walletAdapter = {
          signAndSubmitTransaction: async (payload: {
            data: {
              function: `${string}::${string}::${string}`;
              functionArguments: unknown[];
            };
          }) => {
            const result = await signAndSubmitTransaction(payload as never);
            return { hash: (result as { hash?: string })?.hash ?? "" };
          },
          account: { address: account.address.toString() },
        };

        const provider = new MovementSessionProvider(walletAdapter, providerOpts);
        sessionProviderRef.current = provider;

        const credential = await provider.pay(challenge);
        const authHeader = formatAuthorization(credential);

        // Wait for on-chain confirmation
        await new Promise((r) => setTimeout(r, 2000));

        // 3. Retry /api/call/start with credential + channel info
        const payload = credential.payload as {
          channelId?: string;
          authorizedSigner?: string;
        };
        const retryResp = await fetch(
          `${SERVER_URL}/api/call/start?host=${hostAddress}`,
          {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              Authorization: authHeader,
            },
            body: JSON.stringify({
              address: account.address.toString(),
              channelId: payload.channelId,
              pubkey: payload.authorizedSigner,
            }),
          },
        );

        if (!retryResp.ok) {
          const body = await retryResp.text();
          throw new Error(`Call start failed: ${retryResp.status} ${body}`);
        }

        const callData = await retryResp.json();
        const callId = callData.callId;
        callIdRef.current = callId;

        setCallState("ringing");
        callStateRef.current = "ringing";

        // 4. Get microphone
        const localStream = await getUserAudio();
        localStreamRef.current = localStream;

        // 5. Create peer connection
        const pc = createPeerConnection((stream) => {
          setRemoteStream(stream);
        });
        peerConnectionRef.current = pc;

        // Expose for debugging: check audio stats via browser console with
        //   checkAudio()
        (window as any).__voiceCallPC = pc;
        (window as any).checkAudio = async () => {
          const stats = await pc.getStats();
          stats.forEach((report: any) => {
            if (report.type === "inbound-rtp" && report.kind === "audio") {
              console.log(
                `[audio] bytes received: ${report.bytesReceived}, packets: ${report.packetsReceived}, lost: ${report.packetsLost}`
              );
            }
            if (report.type === "outbound-rtp" && report.kind === "audio") {
              console.log(
                `[audio] bytes sent: ${report.bytesSent}, packets: ${report.packetsSent}`
              );
            }
          });
        };

        // Create data channel for sending vouchers peer-to-peer
        const dataChannel = pc.createDataChannel("vouchers");
        dataChannelRef.current = dataChannel;

        // Add local audio tracks
        localStream.getTracks().forEach((track) => {
          pc.addTrack(track, localStream);
        });

        // 6. Connect signaling
        const wsUrl = `${SERVER_URL.replace(/^http/, "ws")}/ws/signal/${callId}?address=${account.address}`;
        const signaling = connectSignaling(wsUrl, {
          onAnswer(answer) {
            console.log("[caller] received answer");
            pc.setRemoteDescription(new RTCSessionDescription(answer));
          },
          onIceCandidate(candidate) {
            pc.addIceCandidate(new RTCIceCandidate(candidate));
          },
          onError(msg) {
            setError(`Signaling: ${msg}`);
          },
          onClose() {
            if (callStateRef.current === "in_call") {
              hangupInternal();
            }
          },
        });
        signalingRef.current = signaling;

        // Send ICE candidates to peer
        pc.onicecandidate = (event) => {
          if (event.candidate) {
            signaling.send({
              type: "ice-candidate",
              candidate: event.candidate.toJSON(),
            });
          }
        };

        // Create offer
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        // Send offer repeatedly until we get an answer (host may not be connected yet)
        const offerPayload = { type: "offer", offer: pc.localDescription!.toJSON() };
        signaling.send(offerPayload);
        console.log("[caller] sent offer");

        const offerRetry = setInterval(() => {
          if (callStateRef.current !== "ringing") {
            clearInterval(offerRetry);
            return;
          }
          console.log("[caller] resending offer");
          signaling.send(offerPayload);
        }, 2000);

        // Clean up retry when state changes
        const clearOfferRetry = () => clearInterval(offerRetry);

        // When connected, start the call timers
        const handleConnected = () => {
          if (callStateRef.current === "ringing") {
            clearOfferRetry();
            console.log("[caller] connected!");
            setCallState("in_call");
            callStateRef.current = "in_call";
            startTimers();
          }
        };
        const handleDisconnected = () => {
          if (callStateRef.current === "in_call") {
            hangupInternal();
          }
        };

        pc.onconnectionstatechange = () => {
          if (pc.connectionState === "connected") handleConnected();
          else if (pc.connectionState === "disconnected" || pc.connectionState === "failed") handleDisconnected();
        };
        // Also listen to iceConnectionState (more reliable in some browsers)
        pc.oniceconnectionstatechange = () => {
          if (pc.iceConnectionState === "connected" || pc.iceConnectionState === "completed") handleConnected();
          else if (pc.iceConnectionState === "disconnected" || pc.iceConnectionState === "failed") handleDisconnected();
        };

        // Also transition when the data channel opens (proves p2p connection works)
        dataChannel.onopen = () => {
          handleConnected();
        };
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        cleanup();
        setCallState("idle");
        callStateRef.current = "idle";
      }
    },
    [account, signAndSubmitTransaction, cleanup],
  );

  function startTimers() {
    const startTime = Date.now();

    // Duration counter
    durationIntervalRef.current = setInterval(() => {
      const elapsed = Math.floor((Date.now() - startTime) / 1000);
      setDuration(elapsed);
    }, 1000);

    // Voucher loop: every 5 seconds, send a voucher over the data channel
    voucherIntervalRef.current = setInterval(() => {
      if (callStateRef.current !== "in_call") return;

      const provider = sessionProviderRef.current;
      const dc = dataChannelRef.current;
      if (!provider || !dc || dc.readyState !== "open") return;

      try {
        // Delta = rate_per_second * 5 seconds
        const delta = rateRef.current * 5n;
        const { channelId, cumulativeAmount, signature } =
          provider.signVoucherFor(
            hostAddressRef.current,
            TOKEN_METADATA_ADDR,
            delta,
          );

        setTotalPaid(cumulativeAmount);

        dc.send(JSON.stringify({
          channelId: toHex(channelId),
          cumulativeAmount: cumulativeAmount.toString(),
          signature: toHex(signature),
          pubkey: toHex(provider.sessionPublicKey),
        }));
      } catch {
        // Voucher send failed, will retry next interval
      }
    }, 5000);
  }

  async function hangupInternal() {
    if (
      callStateRef.current === "hanging_up" ||
      callStateRef.current === "ended" ||
      callStateRef.current === "idle"
    ) {
      return;
    }
    setCallState("hanging_up");
    callStateRef.current = "hanging_up";

    cleanup();

    try {
      await fetch(`${SERVER_URL}/api/call/hangup`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ callId: callIdRef.current }),
      });
    } catch {
      // Best-effort hangup notification
    }

    setCallState("ended");
    callStateRef.current = "ended";
  }

  const hangup = useCallback(async () => {
    await hangupInternal();
  }, []);

  return {
    callState,
    duration,
    totalPaid,
    error,
    remoteStream,
    startCall,
    hangup,
  };
}
