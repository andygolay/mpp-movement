export interface SignalingHandlers {
  onOffer?: (offer: RTCSessionDescriptionInit) => void;
  onAnswer?: (answer: RTCSessionDescriptionInit) => void;
  onIceCandidate?: (candidate: RTCIceCandidateInit) => void;
  onError?: (error: string) => void;
  onClose?: () => void;
}

export interface SignalingConnection {
  send: (msg: Record<string, unknown>) => void;
  close: () => void;
}

/**
 * Connect to a WebSocket signaling server for WebRTC negotiation.
 */
export function connectSignaling(
  wsUrl: string,
  handlers: SignalingHandlers,
): SignalingConnection {
  const ws = new WebSocket(wsUrl);

  ws.onopen = () => {
    // Connection ready
  };

  ws.onmessage = (event) => {
    try {
      const msg = JSON.parse(event.data as string);

      switch (msg.type) {
        case "offer":
          handlers.onOffer?.(msg.offer);
          break;
        case "answer":
          handlers.onAnswer?.(msg.answer);
          break;
        case "ice-candidate":
          handlers.onIceCandidate?.(msg.candidate);
          break;
        case "error":
          handlers.onError?.(msg.message ?? "Unknown signaling error");
          break;
        default:
          break;
      }
    } catch {
      // Ignore malformed messages
    }
  };

  ws.onclose = () => {
    handlers.onClose?.();
  };

  ws.onerror = () => {
    handlers.onError?.("WebSocket connection error");
  };

  return {
    send(msg: Record<string, unknown>) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify(msg));
      }
    },
    close() {
      ws.close();
    },
  };
}
