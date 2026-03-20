import { useState } from "react";
import { MovementWalletAdapterProvider } from "@moveindustries/wallet-adapter-react";
import { WalletConnect } from "./components/WalletConnect";
import { HostPanel } from "./components/HostPanel";
import { CallerPanel } from "./components/CallerPanel";

type Mode = "caller" | "host";

// Auto-switch to host mode if there's an unsettled voucher
function getInitialMode(): Mode {
  try {
    const voucher = localStorage.getItem("voice-call-unsettled-voucher");
    if (voucher) return "host";
  } catch {}
  return "caller";
}

export default function App() {
  const [mode, setMode] = useState<Mode>(getInitialMode);

  return (
    <MovementWalletAdapterProvider autoConnect>
      <header>
        <h1>Voice Call</h1>
        <div style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
          <div className="mode-toggle">
            <button
              className={mode === "caller" ? "active" : ""}
              onClick={() => setMode("caller")}
            >
              Caller
            </button>
            <button
              className={mode === "host" ? "active" : ""}
              onClick={() => setMode("host")}
            >
              Host
            </button>
          </div>
          <WalletConnect />
        </div>
      </header>

      {mode === "host" ? <HostPanel /> : <CallerPanel />}
    </MovementWalletAdapterProvider>
  );
}
