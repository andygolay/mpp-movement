import { MovementWalletAdapterProvider } from "@moveindustries/wallet-adapter-react";
import { StreamingDemo } from "./components/StreamingDemo";

export default function App() {
  return (
    <MovementWalletAdapterProvider autoConnect>
      <StreamingDemo />
    </MovementWalletAdapterProvider>
  );
}
