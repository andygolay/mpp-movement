import { MovementWalletAdapterProvider } from "@moveindustries/wallet-adapter-react";
import { PipelineDemo } from "./components/PipelineDemo";

export default function App() {
  return (
    <MovementWalletAdapterProvider autoConnect>
      <PipelineDemo />
    </MovementWalletAdapterProvider>
  );
}
