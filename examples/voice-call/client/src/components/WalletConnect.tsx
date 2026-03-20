import { useWallet } from "@moveindustries/wallet-adapter-react";
import { useState } from "react";

export function WalletConnect() {
  const { connected, account, connect, disconnect, wallets } = useWallet();
  const [showPicker, setShowPicker] = useState(false);

  const accountAddr = account?.address?.toString() ?? "";

  if (connected && account) {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: "0.75rem" }}>
        <span className="wallet-info">
          {accountAddr.slice(0, 6)}...{accountAddr.slice(-4)}
        </span>
        <button onClick={() => disconnect()}>Disconnect</button>
      </div>
    );
  }

  return (
    <div style={{ position: "relative" }}>
      <button
        className="primary"
        onClick={() => setShowPicker((prev) => !prev)}
      >
        Connect Wallet
      </button>
      {showPicker && wallets.length > 0 && (
        <div className="wallet-picker">
          {wallets.map((w) => (
            <button
              key={w.name}
              className="wallet-option"
              onClick={() => {
                connect(w.name);
                setShowPicker(false);
              }}
            >
              {w.icon && (
                <img
                  src={w.icon}
                  alt={w.name}
                  width={24}
                  height={24}
                  style={{ borderRadius: 4 }}
                />
              )}
              {w.name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
