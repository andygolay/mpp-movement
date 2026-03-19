import { useCallback, useRef, useState } from "react";
import { useWallet } from "@moveindustries/wallet-adapter-react";
import {
  SERVER_URL,
  MODULE_ADDRESS,
  REGISTRY_ADDR,
  TOKEN_METADATA_ADDR,
  TOKEN_SYMBOL,
  TOKEN_DECIMALS,
  TOKENS_PER_VOUCHER,
} from "../lib/constants";
import {
  signVoucher,
  getPublicKey,
  computeChannelId,
  randomSalt,
  toHex,
  hexToBytes,
} from "../lib/voucher";

type DemoState = "idle" | "opening" | "streaming" | "closing" | "closed";

interface CloseSummary {
  settled: string;
  settleTxns: string[];
  closeTx: string | null;
  vouchersReceived: number;
}

export function StreamingDemo() {
  const {
    connected,
    account,
    connect,
    disconnect,
    wallets,
    signAndSubmitTransaction,
  } = useWallet();

  const [state, setState] = useState<DemoState>("idle");
  const [prompt, setPrompt] = useState("Tell me something interesting");
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [channelIdHex, setChannelIdHex] = useState("");
  const [showWalletPicker, setShowWalletPicker] = useState(false);

  // Payment tracking
  const [cumulativePaid, setCumulativePaid] = useState(0n);
  const [vouchersSent, setVouchersSent] = useState(0);
  const [tokensReceived, setTokensReceived] = useState(0);
  const [pricePerToken, setPricePerToken] = useState(0n);
  const [deposit, setDeposit] = useState(0n);
  const [closeSummary, setCloseSummary] = useState<CloseSummary | null>(null);

  // Refs for streaming loop
  const abortRef = useRef<AbortController | null>(null);
  const sessionKeyRef = useRef<Uint8Array | null>(null);
  const sessionPubKeyRef = useRef<Uint8Array | null>(null);
  const channelIdRef = useRef<Uint8Array | null>(null);
  const channelIdHexRef = useRef("");
  const cumulativeRef = useRef(0n);
  const depositRef = useRef(0n);
  const stateRef = useRef<DemoState>("idle");

  const formatAmount = useCallback((amount: bigint) => {
    const divisor = 10 ** TOKEN_DECIMALS;
    return (Number(amount) / divisor).toFixed(TOKEN_DECIMALS);
  }, []);

  function handleConnect(walletName?: string) {
    if (walletName) {
      connect(walletName);
      setShowWalletPicker(false);
    } else {
      setShowWalletPicker((prev) => !prev);
    }
  }

  async function handleStart() {
    if (!account) return;
    setError("");
    setText("");
    setCumulativePaid(0n);
    setVouchersSent(0);
    setTokensReceived(0);
    setCloseSummary(null);
    cumulativeRef.current = 0n;
    setState("opening");
    stateRef.current = "opening";

    try {
      // 1. Hit /api/chat to get 402 + pricing info.
      const resp = await fetch(
        `${SERVER_URL}/api/chat?prompt=${encodeURIComponent(prompt)}`,
      );
      if (resp.status !== 402) {
        throw new Error(`Expected 402, got ${resp.status}`);
      }
      const body = await resp.json();
      const ppt = BigInt(body.price_per_token);
      const dep = BigInt(body.suggested_deposit);
      const recipient: string = body.recipient;
      setPricePerToken(ppt);
      setDeposit(dep);
      depositRef.current = dep;

      // 2. Generate ephemeral session keypair for voucher signing.
      const sessionPrivKey = crypto.getRandomValues(new Uint8Array(32));
      const sessionPubKey = getPublicKey(sessionPrivKey);
      sessionKeyRef.current = sessionPrivKey;
      sessionPubKeyRef.current = sessionPubKey;

      // 3. Open channel via wallet.
      const salt = randomSalt();
      const payerAddr = account.address.toString();
      const payerBytes = hexToBytes(payerAddr);
      const payeeBytes = hexToBytes(recipient);
      const tokenBytes = hexToBytes(TOKEN_METADATA_ADDR);

      const chId = computeChannelId(
        payerBytes,
        payeeBytes,
        tokenBytes,
        salt,
        sessionPubKey,
      );
      channelIdRef.current = chId;
      const chHex = toHex(chId);
      channelIdHexRef.current = chHex;
      setChannelIdHex(chHex);

      const txResponse = await signAndSubmitTransaction({
        data: {
          function:
            `${MODULE_ADDRESS}::channel::open` as `${string}::${string}::${string}`,
          functionArguments: [
            REGISTRY_ADDR,
            recipient,
            TOKEN_METADATA_ADDR,
            Number(dep),
            Array.from(salt),
            Array.from(sessionPubKey),
          ],
        },
      });

      // Wait a moment for on-chain confirmation.
      if (txResponse?.hash) {
        await new Promise((r) => setTimeout(r, 2000));
      }

      // 4. Start streaming loop.
      setState("streaming");
      stateRef.current = "streaming";
      startStreaming(ppt);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setState("idle");
      stateRef.current = "idle";
    }
  }

  function startStreaming(ppt: bigint) {
    const controller = new AbortController();
    abortRef.current = controller;

    (async () => {
      try {
        while (!controller.signal.aborted) {
          const delta = ppt * BigInt(TOKENS_PER_VOUCHER);
          if (cumulativeRef.current + delta > depositRef.current) {
            setText((prev) => prev + "\n\n[deposit exhausted]");
            break;
          }
          cumulativeRef.current += delta;
          const cumulative = cumulativeRef.current;

          // Sign voucher with session key.
          const sig = signVoucher(
            {
              channelId: channelIdRef.current!,
              cumulativeAmount: cumulative,
            },
            sessionKeyRef.current!,
          );

          setCumulativePaid(cumulative);
          setVouchersSent((n) => n + 1);

          // Request tokens with voucher.
          const url =
            `${SERVER_URL}/api/chat` +
            `?prompt=${encodeURIComponent(prompt)}` +
            `&channel_id=${channelIdHexRef.current}` +
            `&cumulative_amount=${cumulative}` +
            `&signature=${toHex(sig)}` +
            `&pubkey=${toHex(sessionPubKeyRef.current!)}`;

          const resp = await fetch(url, { signal: controller.signal });

          if (!resp.ok) {
            const errBody = await resp.text();
            setText((prev) => prev + `\n\n[server error: ${errBody}]`);
            break;
          }

          // Parse SSE stream.
          const reader = resp.body!.getReader();
          const decoder = new TextDecoder();
          let buffer = "";

          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, { stream: true });

            while (buffer.includes("\n\n")) {
              const pos = buffer.indexOf("\n\n");
              const event = buffer.slice(0, pos);
              buffer = buffer.slice(pos + 2);

              for (const line of event.split("\n")) {
                if (line.startsWith("data: ")) {
                  try {
                    const data = JSON.parse(line.slice(6));
                    if (data.token) {
                      setText((prev) => prev + data.token);
                      setTokensReceived((n) => n + 1);
                    }
                  } catch {
                    // skip malformed JSON
                  }
                }
              }
            }
          }
        }
      } catch (e: unknown) {
        if (e instanceof DOMException && e.name === "AbortError") return;
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
      } finally {
        if (stateRef.current !== "closing") {
          handleStop();
        }
      }
    })();
  }

  async function handleStop() {
    abortRef.current?.abort();
    setState("closing");
    stateRef.current = "closing";

    try {
      // Wait briefly for any in-flight server settlements.
      await new Promise((r) => setTimeout(r, 2000));

      const resp = await fetch(
        `${SERVER_URL}/api/close?channel_id=${channelIdHexRef.current}`,
      );
      const body = await resp.json();

      setCloseSummary({
        settled: body.settled ?? "0",
        settleTxns: body.settle_txns ?? [],
        closeTx: body.close_tx ?? null,
        vouchersReceived: body.vouchers_received ?? 0,
      });
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Close failed: ${msg}`);
    }

    setState("closed");
    stateRef.current = "closed";
  }

  function handleReset() {
    setState("idle");
    stateRef.current = "idle";
    setText("");
    setError("");
    setChannelIdHex("");
    setCumulativePaid(0n);
    setVouchersSent(0);
    setTokensReceived(0);
    setPricePerToken(0n);
    setDeposit(0n);
    setCloseSummary(null);
    cumulativeRef.current = 0n;
  }

  const isStreaming = state === "streaming";
  const isBusy = state === "opening" || state === "closing";
  const accountAddr = account?.address?.toString() ?? "";

  return (
    <>
      <header>
        <h1>
          <span>Tempo</span> Stream Demo
        </h1>
        {connected && account ? (
          <div
            style={{ display: "flex", alignItems: "center", gap: "0.75rem" }}
          >
            <span className="wallet-info">
              {accountAddr.slice(0, 6)}...{accountAddr.slice(-4)}
            </span>
            <button onClick={() => disconnect()}>Disconnect</button>
          </div>
        ) : (
          <div style={{ position: "relative" }}>
            <button className="primary" onClick={() => handleConnect()}>
              Connect Wallet
            </button>
            {showWalletPicker && wallets.length > 0 && (
              <div className="wallet-picker">
                {wallets.map((w) => (
                  <button
                    key={w.name}
                    className="wallet-option"
                    onClick={() => handleConnect(w.name)}
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
        )}
      </header>

      {error && <div className="error">{error}</div>}

      <div className="prompt-row">
        <input
          type="text"
          placeholder="Enter a prompt..."
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          disabled={isStreaming || isBusy}
          onKeyDown={(e) => {
            if (e.key === "Enter" && connected && state === "idle")
              handleStart();
          }}
        />
      </div>

      <div className="stream-box">
        {text ? (
          <>
            {text}
            {isStreaming && <span className="cursor" />}
          </>
        ) : (
          <span className="placeholder">
            {!connected
              ? "Connect your wallet to start"
              : state === "opening"
                ? "Opening payment channel..."
                : state === "closing"
                  ? "Closing channel..."
                  : "AI response will appear here"}
          </span>
        )}
      </div>

      {(state !== "idle" || closeSummary) && (
        <div className="status-panel">
          <div className="status-grid">
            <div className="status-item">
              <label>Cost so far</label>
              <div className="value green">
                {formatAmount(cumulativePaid)} {TOKEN_SYMBOL}
              </div>
            </div>
            <div className="status-item">
              <label>Deposit</label>
              <div className="value">
                {formatAmount(deposit)} {TOKEN_SYMBOL}
              </div>
            </div>
            <div className="status-item">
              <label>Tokens received</label>
              <div className="value blue">{tokensReceived}</div>
            </div>
            <div className="status-item">
              <label>Vouchers sent</label>
              <div className="value">{vouchersSent}</div>
            </div>
            {pricePerToken > 0n && (
              <div className="status-item">
                <label>Price per token</label>
                <div className="value">
                  {formatAmount(pricePerToken)} {TOKEN_SYMBOL}
                </div>
              </div>
            )}
            {channelIdHex && (
              <div className="status-item">
                <label>Channel ID</label>
                <div className="value" style={{ fontSize: "0.7rem" }}>
                  {channelIdHex.slice(0, 10)}...{channelIdHex.slice(-8)}
                </div>
              </div>
            )}
          </div>

          {closeSummary && (
            <div className="txn-list">
              <h3>
                On-chain transactions ({closeSummary.settleTxns.length} settles
                {closeSummary.closeTx ? " + 1 close" : ""})
              </h3>
              {closeSummary.settleTxns.map((tx, i) => (
                <a
                  key={tx}
                  href={`https://explorer.movementnetwork.xyz/txn/${tx}?network=testnet`}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  settle #{i + 1}: {tx}
                </a>
              ))}
              {closeSummary.closeTx && (
                <a
                  href={`https://explorer.movementnetwork.xyz/txn/${closeSummary.closeTx}?network=testnet`}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  close: {closeSummary.closeTx}
                </a>
              )}
            </div>
          )}
        </div>
      )}

      <div className="controls">
        {state === "idle" && (
          <button
            className="primary"
            disabled={!connected || !prompt.trim()}
            onClick={handleStart}
          >
            Start Streaming
          </button>
        )}
        {isStreaming && (
          <button className="danger" onClick={handleStop}>
            Stop
          </button>
        )}
        {state === "closed" && (
          <button onClick={handleReset}>New Session</button>
        )}
        {isBusy && (
          <button disabled>
            {state === "opening" ? "Opening channel..." : "Closing channel..."}
          </button>
        )}
      </div>
    </>
  );
}
