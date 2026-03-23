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

type PipelineState =
  | "idle"
  | "challenging"
  | "paying"
  | "searching"
  | "generating"
  | "emailing"
  | "done"
  | "error";

interface SearchResult {
  title: string;
  url: string;
  summary: string;
}

interface PipelineResult {
  search_results: SearchResult[];
  image_url: string | null;
  email_sent_to: string | null;
  email_id: string | null;
  steps_completed: string[];
  partial_failure: string | null;
}

export function PipelineDemo() {
  const { connected, account, connect, disconnect, wallets, signAndSubmitTransaction } =
    useWallet();

  const [query, setQuery] = useState("things to do in San Francisco in April 2026");
  const [email, setEmail] = useState("");
  const [state, setState] = useState<PipelineState>("idle");
  const [result, setResult] = useState<PipelineResult | null>(null);
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

  const handleRun = useCallback(async () => {
    if (!account) return;
    setError("");
    setResult(null);
    setTxHash("");
    setChargeInfo(null);

    const body = { query, email };

    try {
      setState("challenging");
      const resp = await fetch(`${SERVER_URL}/api/run`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });

      if (resp.status !== 402) {
        if (resp.ok) {
          setResult(await resp.json());
          setState("done");
          return;
        }
        const errBody = await resp.json().catch(() => ({}));
        throw new Error(errBody.error || `Unexpected status ${resp.status}`);
      }

      const wwwAuth = resp.headers.get("www-authenticate");
      if (!wwwAuth) throw new Error("No WWW-Authenticate header");

      const challenge: PaymentChallenge = parseWwwAuthenticate(wwwAuth);
      const chargeReq = decodeChargeRequest(challenge);
      setChargeInfo(chargeReq);

      if (!chargeReq.recipient) throw new Error("No recipient in charge request");

      setState("paying");
      const amount = BigInt(chargeReq.amount);
      const isNativeMove =
        chargeReq.currency === "0xa" ||
        chargeReq.currency === "0x000000000000000000000000000000000000000000000000000000000000000a";

      const txPayload = isNativeMove
        ? {
            function: "0x1::aptos_account::transfer" as `${string}::${string}::${string}`,
            functionArguments: [chargeReq.recipient, Number(amount)],
          }
        : {
            function: "0x1::primary_fungible_store::transfer" as `${string}::${string}::${string}`,
            typeArguments: ["0x1::fungible_asset::Metadata"],
            functionArguments: [chargeReq.currency, chargeReq.recipient, Number(amount)],
          };

      const txResponse = await signAndSubmitTransaction({ data: txPayload });
      if (!txResponse?.hash) throw new Error("Transaction failed");
      setTxHash(txResponse.hash);

      await waitForTransaction(txResponse.hash);

      setState("searching");
      const authHeader = buildAuthorizationHeader(
        challenge,
        txResponse.hash,
        account.address.toString(),
      );

      const statusInterval = setInterval(() => {
        setState((prev) => {
          if (prev === "searching") return "generating";
          if (prev === "generating") return "emailing";
          return prev;
        });
      }, 5000);

      const retryResp = await fetch(`${SERVER_URL}/api/run`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: authHeader,
        },
        body: JSON.stringify(body),
      });

      clearInterval(statusInterval);

      if (!retryResp.ok) {
        const errBody = await retryResp.json().catch(() => ({}));
        throw new Error(errBody.error || `Pipeline failed (${retryResp.status})`);
      }

      setResult(await retryResp.json());
      setState("done");
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setState("error");
    }
  }, [account, query, email, signAndSubmitTransaction]);

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
    setResult(null);
    setError("");
    setTxHash("");
    setChargeInfo(null);
  }

  const accountAddr = account?.address?.toString() ?? "";
  const isBusy = !["idle", "done", "error"].includes(state);

  const statusText = (() => {
    switch (state) {
      case "challenging": return "Getting price...";
      case "paying": return "Confirm in wallet...";
      case "searching": return "Searching the web...";
      case "generating": return "Generating image...";
      case "emailing": return "Sending email...";
      default: return "";
    }
  })();

  return (
    <div className="app">
      {/* Top bar */}
      <nav className="topbar">
        <div className="logo">
          <span className="logo-search">Search</span>
          <span className="logo-arrow">&rarr;</span>
          <span className="logo-image">Image</span>
          <span className="logo-arrow">&rarr;</span>
          <span className="logo-email">Email</span>
        </div>
        <div className="topbar-right">
          {connected && account ? (
            <div className="wallet-row">
              <span className="wallet-info">
                {accountAddr.slice(0, 6)}...{accountAddr.slice(-4)}
              </span>
              <button className="btn-sm" onClick={() => disconnect()}>
                Disconnect
              </button>
            </div>
          ) : (
            <div style={{ position: "relative" }}>
              <button className="btn-primary" onClick={() => handleConnect()}>
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
        </div>
      </nav>

      {/* Main content */}
      <main className="main">
        <div className="hero">
          <p className="tagline">
            Search the web, generate an AI image, and send the results as an email
            — powered by MPP on Movement
          </p>
        </div>

        {error && <div className="error">{error}</div>}

        {/* Input card */}
        <div className="card">
          <div className="field">
            <label>Search query</label>
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              disabled={isBusy}
              placeholder="things to do in San Francisco in April"
            />
          </div>
          <div className="field">
            <label>Send results to</label>
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={isBusy}
              placeholder="you@example.com"
            />
          </div>
          <div className="actions">
            {state === "idle" && (
              <button
                className="btn-primary btn-lg"
                disabled={!connected || !query.trim() || !email.includes("@")}
                onClick={handleRun}
              >
                {!connected ? "Connect wallet to start" : "Run — 0.05 MOVE"}
              </button>
            )}
            {(state === "done" || state === "error") && (
              <button className="btn-secondary btn-lg" onClick={handleReset}>
                New Search
              </button>
            )}
            {isBusy && (
              <button className="btn-primary btn-lg" disabled>
                <span className="spinner" />
                {statusText}
              </button>
            )}
          </div>
        </div>

        {/* Flow visualization */}
        {state !== "idle" && (
          <div className="flow">
            <div
              className={`flow-step ${
                state === "challenging" || state === "paying"
                  ? "active"
                  : txHash
                    ? "done"
                    : ""
              }`}
            >
              <div className="flow-num">1</div>
              <div>Pay</div>
            </div>
            <div className="flow-connector" />
            <div
              className={`flow-step ${
                state === "searching"
                  ? "active"
                  : result || state === "generating" || state === "emailing"
                    ? "done"
                    : ""
              }`}
            >
              <div className="flow-num">2</div>
              <div>Search</div>
            </div>
            <div className="flow-connector" />
            <div
              className={`flow-step ${
                state === "generating"
                  ? "active"
                  : result?.steps_completed.includes("image")
                    ? "done"
                    : result
                      ? "failed"
                      : ""
              }`}
            >
              <div className="flow-num">3</div>
              <div>Image</div>
            </div>
            <div className="flow-connector" />
            <div
              className={`flow-step ${
                state === "emailing"
                  ? "active"
                  : result?.steps_completed.includes("email")
                    ? "done"
                    : result
                      ? "failed"
                      : ""
              }`}
            >
              <div className="flow-num">4</div>
              <div>Email</div>
            </div>
          </div>
        )}

        {/* Payment info */}
        {txHash && (
          <div className="card card-compact">
            <div className="payment-row">
              {chargeInfo && (
                <span>
                  Paid{" "}
                  <strong>
                    {(
                      Number(chargeInfo.amount) /
                      10 ** (chargeInfo.decimals ?? 8)
                    ).toFixed(chargeInfo.decimals ?? 8)}{" "}
                    MOVE
                  </strong>
                </span>
              )}
              <a
                href={`https://explorer.movementnetwork.xyz/txn/${txHash}?network=testnet`}
                target="_blank"
                rel="noopener noreferrer"
              >
                View transaction
              </a>
            </div>
          </div>
        )}

        {/* Results */}
        {result && (
          <>
            {result.partial_failure && (
              <div className="warning">
                Some steps didn't complete: {result.partial_failure}
              </div>
            )}

            {result.image_url && (
              <div className="card card-image">
                <img src={result.image_url} alt="AI generated" />
              </div>
            )}

            <div className="card">
              <div className="card-header">
                <h2>Search Results</h2>
                {result.email_sent_to && (
                  <span className="badge badge-green">
                    Emailed to {result.email_sent_to}
                  </span>
                )}
                {!result.email_sent_to && (
                  <span className="badge badge-red">Email failed</span>
                )}
              </div>
              <div className="results-list">
                {result.search_results.map((r, i) => (
                  <div key={i} className="result-item">
                    <div className="result-num">{i + 1}</div>
                    <div className="result-body">
                      <a
                        href={r.url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="result-title"
                      >
                        {r.title}
                      </a>
                      <p className="result-summary">{r.summary}</p>
                      <span className="result-url">{r.url}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </>
        )}

        {/* How it works */}
        {state === "idle" && !result && (
          <div className="card how-it-works">
            <h2>How it works</h2>
            <div className="steps-grid">
              <div className="step">
                <div className="step-num">1</div>
                <div>
                  <strong>Connect</strong>
                  <p>Link your Movement wallet</p>
                </div>
              </div>
              <div className="step">
                <div className="step-num">2</div>
                <div>
                  <strong>Pay once</strong>
                  <p>0.05 MOVE covers all three services</p>
                </div>
              </div>
              <div className="step">
                <div className="step-num">3</div>
                <div>
                  <strong>Exa searches</strong>
                  <p>AI-powered web search for your query</p>
                </div>
              </div>
              <div className="step">
                <div className="step-num">4</div>
                <div>
                  <strong>fal.ai creates</strong>
                  <p>Generates an image from the results</p>
                </div>
              </div>
              <div className="step">
                <div className="step-num">5</div>
                <div>
                  <strong>Resend delivers</strong>
                  <p>Emails everything to your inbox</p>
                </div>
              </div>
            </div>
            <p className="footnote">
              Three paid APIs, one MOVE payment, no accounts needed anywhere.
            </p>
          </div>
        )}
      </main>
    </div>
  );
}
