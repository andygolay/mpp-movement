import { useCallback, useState } from "react";
import { useWallet } from "@moveindustries/wallet-adapter-react";
import { SERVER_URL, REST_URL } from "../lib/constants";
import {
  parseWwwAuthenticate,
  decodeChargeRequest,
  buildAuthorizationHeader,
  type PaymentChallenge,
  type ChargeRequest,
} from "../lib/mpp";

type ScrapeState = "idle" | "challenging" | "paying" | "scraping" | "done" | "error";

export function ScrapeDemo() {
  const { connected, account, connect, disconnect, wallets, signAndSubmitTransaction } =
    useWallet();

  const [url, setUrl] = useState("https://example.com");
  const [state, setState] = useState<ScrapeState>("idle");
  const [result, setResult] = useState("");
  const [error, setError] = useState("");
  const [showWalletPicker, setShowWalletPicker] = useState(false);
  const [txHash, setTxHash] = useState("");
  const [chargeInfo, setChargeInfo] = useState<ChargeRequest | null>(null);

  function handleConnect(walletName?: string) {
    if (walletName) {
      connect(walletName);
      setShowWalletPicker(false);
    } else {
      setShowWalletPicker((prev) => !prev);
    }
  }

  const handleScrape = useCallback(async () => {
    if (!account) return;
    setError("");
    setResult("");
    setTxHash("");
    setChargeInfo(null);

    try {
      // Step 1: Send request, expect 402
      setState("challenging");
      const resp = await fetch(`${SERVER_URL}/api/scrape`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ url }),
      });

      if (resp.status !== 402) {
        // If not 402, maybe already paid or error
        if (resp.ok) {
          const data = await resp.json();
          setResult(data?.data?.markdown || JSON.stringify(data, null, 2));
          setState("done");
          return;
        }
        throw new Error(`Expected 402, got ${resp.status}`);
      }

      // Step 2: Parse the 402 challenge
      const wwwAuth = resp.headers.get("www-authenticate");
      if (!wwwAuth) throw new Error("No WWW-Authenticate header in 402 response");

      const challenge: PaymentChallenge = parseWwwAuthenticate(wwwAuth);
      const chargeReq = decodeChargeRequest(challenge);
      setChargeInfo(chargeReq);

      if (!chargeReq.recipient) throw new Error("No recipient in charge request");

      // Step 3: Pay on-chain
      setState("paying");
      const amount = BigInt(chargeReq.amount);
      const recipient = chargeReq.recipient;
      const currency = chargeReq.currency;
      const isNativeMove = currency === "0xa" || currency === "0x000000000000000000000000000000000000000000000000000000000000000a";

      const txPayload = isNativeMove
        ? {
            function: "0x1::aptos_account::transfer" as `${string}::${string}::${string}`,
            functionArguments: [recipient, Number(amount)],
          }
        : {
            function: "0x1::primary_fungible_store::transfer" as `${string}::${string}::${string}`,
            typeArguments: ["0x1::fungible_asset::Metadata"],
            functionArguments: [currency, recipient, Number(amount)],
          };

      const txResponse = await signAndSubmitTransaction({ data: txPayload });

      if (!txResponse?.hash) throw new Error("Transaction failed — no hash returned");

      setTxHash(txResponse.hash);

      // Wait for on-chain confirmation
      await waitForTransaction(txResponse.hash);

      // Step 4: Retry with payment proof
      setState("scraping");
      const authHeader = buildAuthorizationHeader(
        challenge,
        txResponse.hash,
        account.address.toString(),
      );

      const retryResp = await fetch(`${SERVER_URL}/api/scrape`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: authHeader,
        },
        body: JSON.stringify({ url }),
      });

      if (!retryResp.ok) {
        const errBody = await retryResp.text();
        throw new Error(`Retry failed (${retryResp.status}): ${errBody}`);
      }

      const data = await retryResp.json();
      const markdown = data?.data?.markdown || data?.data?.content || JSON.stringify(data, null, 2);
      setResult(markdown);
      setState("done");
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setState("error");
    }
  }, [account, url, signAndSubmitTransaction]);

  async function waitForTransaction(hash: string) {
    for (let i = 0; i < 30; i++) {
      try {
        const resp = await fetch(`${REST_URL}/transactions/by_hash/${hash}`);
        if (resp.ok) {
          const tx = await resp.json();
          if (tx.success !== undefined) return;
        }
      } catch {
        // retry
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
    throw new Error("Transaction confirmation timeout");
  }

  function handleReset() {
    setState("idle");
    setResult("");
    setError("");
    setTxHash("");
    setChargeInfo(null);
  }

  const accountAddr = account?.address?.toString() ?? "";
  const isBusy = state !== "idle" && state !== "done" && state !== "error";

  return (
    <>
      <header>
        <h1>
          <span>MPP Services</span> — Firecrawl
        </h1>
        <p className="subtitle">Pay-per-scrape web data extraction on Movement</p>
        {connected && account ? (
          <div className="wallet-row">
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
                      <img src={w.icon} alt={w.name} width={24} height={24} style={{ borderRadius: 4 }} />
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

      <div className="input-section">
        <label>URL to scrape</label>
        <div className="input-row">
          <input
            type="url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            disabled={isBusy}
            placeholder="https://example.com"
            onKeyDown={(e) => {
              if (e.key === "Enter" && connected && state === "idle") handleScrape();
            }}
          />
          {state === "idle" && (
            <button
              className="primary"
              disabled={!connected || !url.trim()}
              onClick={handleScrape}
            >
              Scrape — 0.01 MOVE
            </button>
          )}
          {(state === "done" || state === "error") && (
            <button onClick={handleReset}>New Scrape</button>
          )}
          {isBusy && (
            <button disabled>
              {state === "challenging" && "Getting price..."}
              {state === "paying" && "Confirm in wallet..."}
              {state === "scraping" && "Scraping..."}
            </button>
          )}
        </div>
      </div>

      {/* Status */}
      {(chargeInfo || txHash) && (
        <div className="status-panel">
          {chargeInfo && (
            <div className="status-item">
              <label>Payment</label>
              <div className="value">
                {(Number(chargeInfo.amount) / 10 ** (chargeInfo.decimals ?? 8)).toFixed(
                  chargeInfo.decimals ?? 8,
                )}{" "}
                MOVE → {chargeInfo.recipient?.slice(0, 10)}...
              </div>
            </div>
          )}
          {txHash && (
            <div className="status-item">
              <label>Transaction</label>
              <a
                href={`https://explorer.movementnetwork.xyz/txn/${txHash}?network=testnet`}
                target="_blank"
                rel="noopener noreferrer"
              >
                {txHash.slice(0, 16)}...
              </a>
            </div>
          )}
        </div>
      )}

      {/* Flow visualization */}
      {state !== "idle" && (
        <div className="flow">
          <div className={`flow-step ${state === "challenging" ? "active" : chargeInfo ? "done" : ""}`}>
            1. Request → 402
          </div>
          <div className={`flow-step ${state === "paying" ? "active" : txHash ? "done" : ""}`}>
            2. Pay on-chain
          </div>
          <div className={`flow-step ${state === "scraping" ? "active" : result ? "done" : ""}`}>
            3. Retry → Content
          </div>
        </div>
      )}

      {/* Result */}
      {result && (
        <div className="result-box">
          <label>Scraped content</label>
          <div className="content">{result}</div>
        </div>
      )}

      {/* How it works */}
      {state === "idle" && !result && (
        <div className="info">
          <h2>How it works</h2>
          <ol>
            <li>Connect your Movement wallet</li>
            <li>Enter a URL and click Scrape</li>
            <li>The server responds with HTTP 402 — payment required</li>
            <li>Your wallet signs a 0.01 MOVE transfer on Movement testnet</li>
            <li>The request retries with proof of payment</li>
            <li>Server verifies on-chain, forwards to Firecrawl, returns content</li>
          </ol>
          <p>
            No Firecrawl account needed. No API key. Just a wallet and MOVE tokens.
          </p>
        </div>
      )}
    </>
  );
}
