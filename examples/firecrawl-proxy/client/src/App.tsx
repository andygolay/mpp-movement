import { MovementWalletAdapterProvider } from "@moveindustries/wallet-adapter-react";
import { ScrapeDemo } from "./components/ScrapeDemo";

export default function App() {
  return (
    <MovementWalletAdapterProvider autoConnect>
      <ScrapeDemo />
    </MovementWalletAdapterProvider>
  );
}
