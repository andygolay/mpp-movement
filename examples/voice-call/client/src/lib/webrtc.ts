const ICE_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.l.google.com:19302" },
];

/**
 * Create an RTCPeerConnection configured for audio-only calls.
 * @param onTrack - called when a remote audio track arrives
 */
export function createPeerConnection(
  onTrack: (stream: MediaStream) => void,
): RTCPeerConnection {
  const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });

  pc.ontrack = (event) => {
    if (event.streams[0]) {
      onTrack(event.streams[0]);
    }
  };

  return pc;
}

/**
 * Get user audio media stream (microphone).
 */
export async function getUserAudio(): Promise<MediaStream> {
  return navigator.mediaDevices.getUserMedia({ audio: true, video: false });
}
