import { TURN_URL, TURN_USERNAME, TURN_CREDENTIAL } from "./constants";

function getIceServers(): RTCIceServer[] {
  const servers: RTCIceServer[] = [
    { urls: "stun:stun.l.google.com:19302" },
    { urls: "stun:stun1.l.google.com:19302" },
  ];

  if (TURN_URL) {
    servers.push({
      urls: TURN_URL,
      username: TURN_USERNAME,
      credential: TURN_CREDENTIAL,
    });
  }

  return servers;
}

/**
 * Create an RTCPeerConnection configured for audio-only calls.
 * @param onTrack - called when a remote audio track arrives
 */
export function createPeerConnection(
  onTrack: (stream: MediaStream) => void,
): RTCPeerConnection {
  const pc = new RTCPeerConnection({ iceServers: getIceServers() });

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
