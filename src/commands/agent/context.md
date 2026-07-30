# qn — usage guide for agents

`qn` is the Quicknode command-line interface. It manages endpoints, streams,
webhooks, the KV store, usage, metrics, billing, and teams.

This guide describes qn v{{VERSION}}. It prints as Markdown by default — no flag
needed to read it. For a structured envelope (`{version, guide}`), pass `-o json`.
Read the control-flow sections (auth, output, exit codes, confirmation, retry)
before the command catalog: they decide whether you can run unattended without
hanging or double-acting.

## 1. Auth

Resolution order for the API key:

1. `--api-key <KEY>` flag (highest precedence).
2. Config file: `[api] key = "..."` in `~/.config/qn/config.toml` (or the path
   passed to `--config-file`).
3. If neither resolves, the command exits **4** (`no API key found`).

There is **no environment-variable fallback** by design — a key left exported in
a shell is invisible state that outlives the session.

Non-interactive paths:

- Pass `--api-key <KEY>` on every invocation, or
- Write the key once: `qn auth login --api-key <KEY>` (saves the config file).

`qn auth logout` removes the saved API key but preserves your `[output]`
preferences in the config file.

Config file location:

- Linux/macOS: `$XDG_CONFIG_HOME/qn/config.toml`, else `~/.config/qn/config.toml`.
- Windows: `%USERPROFILE%\.config\qn\config.toml`.

Verify the resolved key against the API: `qn auth whoami` (prints the key redacted
to `****<last4>`, the account id/name and plan, and confirms it works). `qn auth
status` does the same without the network call (no account details).

## 2. Output contract

- Default format is `table` on a TTY and **`json`** when stdout is not a TTY (piped).
- Data goes to **stdout**; diagnostics, prompts, and ✓ confirmations go to **stderr**.
- Formats: `table`, `md`, `json`, `yaml`, `toon`. The structured forms
  (`json`/`yaml`/`toon`) always include every field — `--wide` is not needed and
  only affects `table`/`md`.
- Config file can set defaults: `[output] format = "json"`, `wide = true`.

## 3. Exit codes

Branch on these — especially **4** and **5**.

| Code | Meaning |
|------|---------|
| 0 | Success. |
| 1 | Generic CLI error (bad arguments, I/O, unclassified failure). |
| 2 | API error — the server returned a non-2xx response. |
| 3 | HTTP error — network failure (connect/timeout). |
| 4 | Auth/config — no API key, or a config file that can't be read or written. |
| 5 | Cancelled, or confirmation required and not granted (see §4). |
| 130 | Interrupted (SIGINT). |

On a **per-request paid** `rpc call` (`--x402`/`--mpp`, §6) the 2/3 split carries
payment semantics: **2** means the gateway refused and nothing settled (an
unmatched or unreadable offer, or a 4xx-refused payment), while **3** means the
outcome is unknown — the payment was submitted and **may have been charged** (a
gateway 5xx after the paid resend, a lost response, or an uninterpretable
post-payment response). On exit 3, check the wallet before re-running; never
blind-retry a paid call. A **drawdown** call (`--x402-drawdown`) spends prepaid
credits, not per-call funds: running out surfaces an actionable exit-1 error
pointing at `qn rpc x402 buy-credits`, and a credit is drawn only on success.

## 4. Non-interactive & confirmation behavior

Destructive commands are gated. On a TTY they prompt `y/N`. To proceed without a
prompt, pass `--yes` (`-y`).

In a non-TTY **a gated command without `--yes` exits 5 before any request is sent** —
nothing is changed. Pass `-y` to proceed, or `--no-input` to force non-interactive
behavior everywhere (it fails fast instead of prompting). `--quiet` (`-q`) suppresses
the ✓ state-change notes on stderr; it does not affect stdout.

Gated command classes:

- `endpoint archive`, `endpoint bulk pause`
- `endpoint tag delete`
- `endpoint security` removals (token/jwt/ip/referrer/domain-mask remove, and
  `set-options` toggles that disable a protection)
- `endpoint rate-limit delete-override`
- `stream delete`, `webhook delete`, `team delete`
- `kv set delete`, `kv list delete`

There is **no account-wide wipe command** — that is intentional; use the API directly
if you need it.

## 5. Retry & idempotency

- **Read-only** commands auto-retry transient failures (HTTP 429/500/502/503/504 and
  connect/timeout errors) with exponential backoff and jitter. Tune with `--retries N`
  (default 3; `0` = a single attempt, no retries).
- **Mutations never auto-retry.** A retried create/update/delete could apply twice.
- When a mutation fails transiently, its outcome is unknown until verified — e.g.
  `qn endpoint show <id>` reflects whether it took effect.
- `qn stream test-filter` evaluates a filter against historical data and changes
  nothing — it is read-only and safe to retry.
- `qn sql query` is read-only but **does not auto-retry**: a query consumes credits,
  so a retried query re-bills. `qn sql schema` is a cheap read and retries normally.
- A **paid** `rpc call` (`--x402`/`--mpp`/`--x402-drawdown`/`--mpp-session`)
  never auto-retries — `--retries` does not apply. A per-request attempt can
  move funds, and after a lost response the previous attempt may already have
  settled (§3, exit 3). A drawdown call draws 1 credit per success and is
  single-attempt (the one exception is a transparent re-auth when the session
  token expired, which draws nothing). A session call signs one cumulative
  voucher and is single-attempt.

## 6. Command catalog

Top-level nouns (plurals like `endpoints`/`streams` and `ls` are accepted aliases):

- `auth` — login, logout, whoami, status
- `endpoint` — list, show, create, update, archive, pause, resume, urls, logs,
  log-details, metrics, enable-multichain, disable-multichain; nested:
  `tag`, `security`, `rate-limit`, `bulk`
- `team` — list, create, show, delete, endpoints, set-endpoints; nested: `member`
- `usage` — summary, by-endpoint, by-method, by-chain, by-tag
- `metrics` — account, endpoint
- `chain` — list, credits
- `billing` — invoices, payments
- `stream` — list, show, create, update, delete, activate, pause, test-filter,
  enabled-count
- `webhook` — list, show, create, update, update-template, delete, activate, pause,
  enabled-count
- `kv` — `set` (put, get, list, delete, bulk) and `list` (list, get, create, append,
  contains, remove-item, update, delete)
- `sql` — query (inline SQL, `--file <path>`, or `--file -` for stdin), schema
- `tooling-access` — status, enable, disable (provisions the endpoint `rpc` uses)
- `rpc` — make JSON-RPC calls. `qn rpc call <method> [json-params]` calls the
  account's Tooling Access endpoint (params is a JSON array or object inline, or
  `--params-file <PATH>` / `-f` to read from a file, or `-` for stdin); the
  session JWT is minted and refreshed automatically. On a
  not-yet-enabled account it auto-enables with `--yes` (or prompts on a TTY).
  Multichain: `--network <key>` targets a specific chain by its key (e.g.
  `solana-mainnet`, `polygon`); `qn rpc list-networks` (alias `ls`) lists the
  available keys. Custom endpoint: `--endpoint-url <URL>` (or `[rpc] endpoint_url`
  in config) sends the call to a fully-formed HTTP URL that authenticates itself
  (no token minted); it's mutually exclusive with `--network`.
  **Paid lane**: `--x402` (EVM/Solana stablecoin) or `--mpp` (Tempo) pays for
  the call per request with a crypto micropayment instead of an API key — no
  login, no Tooling Access. Requires `--network` as the payment gateway's path
  slug (e.g. `base-sepolia`; NOT validated by `list-networks`). Parameters:
  `--payment-network <NETWORK>` (the chain the payment settles on — independent of
  `--network`; a network name like `base-sepolia`/`solana-devnet`/`tempo-testnet`,
  or a raw CAIP-2 id like `eip155:84532`, which always passes through
  verbatim), `--payment-asset <ADDRESS>` (a token contract/mint, or the symbol
  `USDC`, resolved to that network's address), `--max-amount <BASE_UNITS>` (spend ceiling
  per call, integer base units, no default), and the private key via
  `--payment-key-file <PATH|->` > `--payment-wallet <NAME>` > `key_file` >
  `wallet` under `[rpc.payment]` in config. The key always comes from a file
  or a stored wallet — never an env var and never a raw key on the command
  line. All parameters fall back to `[rpc.payment]`, but config never
  activates payment by itself: the scheme flag is always required. `--receipt`
  wraps stdout as `{"result": ..., "payment_receipt": ...}` (on MPP an object
  whose `reference` is the settlement tx hash; `null` on x402); without it the
  paid output shape is identical to an unpaid call.
  Mutually exclusive with `--endpoint-url`.
  **Wallets**: stored payment wallets are managed by the top-level `qn wallet`
  noun (see below); reference one on a paid call with `--payment-wallet <NAME>`.
  **Discovery** (per gateway, no API key): `qn rpc {x402,mpp}
  supported-networks` (alias `networks`) lists the networks you can make paid
  calls to — each slug is a valid `--network`. `qn rpc {x402,mpp}
  supported-payments` (alias `payments`) lists the payment options the gateway
  accepts — each row's network/address pair is a ready
  `--payment-network`/`--payment-asset`.
  **x402 drawdown**: `qn rpc x402 {buy-credits, balance, drip}` manages prepaid
  gateway credits instead of paying per request, and `qn rpc call
  --x402-drawdown` spends them (no per-call signing, 1 credit per successful
  response). `buy-credits` signs a payment, so it takes the full paid-lane
  stack (`--payment-wallet`/`-key-file`, `--payment-network`, `--payment-asset`,
  `--max-amount`, `--svm-rpc-url`); `balance` and `drip` only present a Bearer
  session JWT and sign nothing, so they take just the wallet +
  `--payment-network` (+ `--svm-rpc-url` for Solana) and do NOT accept
  `--payment-asset`/`--max-amount`. All fall back to `[rpc.payment]`.
  `--x402-drawdown` requires `--network` (the query chain) and a wallet only —
  it signs nothing per call, so `--payment-asset`/`--max-amount` aren't needed
  and the pay network defaults to `--network`; mutually exclusive with
  `--x402`/`--mpp`/`--endpoint-url`. `buy-credits` takes `--network` (the
  gateway path chain the purchase settles on), SIWX-authenticates, then pays the
  gateway's credit offer (gated Mild — `--yes`, or exit 5 in scripts — and names
  the spend ceiling); `balance` (alias
  `credits`) prints the current credit count (bare number, or the full envelope
  with `--format json`); `drip` funds the wallet from the testnet faucet
  (Base Sepolia, returns the funding tx, not credits; once per account). The
  session JWT is authenticated once and cached under
  `<config-dir>/qn/sessions.toml` (0600, keyed by wallet address); a
  missing/expired session re-authenticates transparently (free, no
  confirmation), including one automatic re-auth if a drawdown call's token
  expired mid-use. Out of credits surfaces an actionable error pointing at
  `qn rpc x402 buy-credits`; nothing auto-retries a credit-drawing call.
  **MPP session**: `qn rpc mpp {open, top-up, close, status}` manages an
  on-chain escrow payment channel (Tempo), and `qn rpc call --mpp-session` pays
  from it with a cumulative EIP-712 voucher (no on-chain tx per call). All take
  the same payment param stack (a Tempo wallet) plus `--network` (the query
  chain the channel lives on). `open --deposit <BASE_UNITS>` and
  `top-up --deposit <BASE_UNITS>` move real funds on-chain (gated Mild); `close`
  cooperatively settles + refunds (gated Mild; its prompt warns further
  `--mpp-session` calls fail until re-open); `status` shows the gateway's view
  and re-syncs the accepted spend into the local record. Channel state is cached
  under `<config-dir>/qn/channels.toml` (0600, keyed by wallet address +
  network); every channel verb needs that record — a lost one means opening a
  new channel. `--mpp-session` requires an open channel, is mutually exclusive with
  `--x402`/`--mpp`/`--x402-drawdown`/`--endpoint-url`, and points at
  `qn rpc mpp top-up` when the channel deposit is exhausted.
- `wallet` — the local store of payment wallets for the paid RPC lane; no API
  key or login required. `qn wallet generate --vm <evm|svm> --name <NAME>`
  creates and stores a dedicated payment wallet (raw key at 0600 under
  `<config-dir>/qn/wallets/`, `evm` also covers MPP/Tempo), printing its
  address (and a QR to fund it on a terminal); `qn wallet list`/`show <NAME>`
  display stored wallets (address only, never the key) and the key file path;
  `qn wallet rm <NAME>` deletes one (gated: `--yes`, or exit 5 in scripts).
  Reference a wallet on a paid call with `--payment-wallet <NAME>`. These
  wallets live only on this machine — Quicknode does not hold, back up, or
  recover them; backing up the key file is the user's job.

Drill into any level with `--help`: `qn endpoint --help`, `qn endpoint security --help`,
`qn endpoint rate-limit --help`. Shell completions: `qn completions <bash|zsh|fish|...>`.

## 7. Common workflows

Capture the `id` (and any URL) from each create response and chain it into the next call.
Run `show` before a state change so you act on the current state, not an assumed one.

**Provision an endpoint and rate-limit it:**

```sh
qn endpoint create --chain ethereum --network mainnet   # → id, http_url, wss_url
qn endpoint show <id>                                    # inspect before modifying
qn endpoint rate-limit set <id> --rps 50
qn endpoint show <id>                                    # verify
```

**Create a stream (paused), inspect it, then activate:**

```sh
qn stream create --name my-stream --network ethereum-mainnet \
  --dataset block --start 15301579 --end 25301589 \
  --batch-size 2 --fix-block-reorgs 1 \
  --notification-email you@example.com --status paused \
  --webhook https://hook.example.com --region usa-east   # → id
qn stream show <id>                                       # inspect while paused
qn stream activate <id>
```

**Create a webhook from a template:**

```sh
qn webhook create --name wallet-watch --network ethereum-mainnet \
  --url https://hook.example.com --compression none --template evm-wallet \
  --wallet 0xabc...                                      # → id
qn webhook show <id>                                     # inspect before activating
qn webhook activate <id>
```

`--compression` (`gzip` or `none`) is required on create. Instead of inline
values, a template can reference a saved list with the matching
`--*-list-name` flag (e.g. `--wallets-list-name`, `--accounts-list-name`,
`--contracts-list-name`); supply either the inline flag or the list-name flag,
not both.

**KV put / get / list:**

```sh
qn kv set put my-key my-value
qn kv set get my-key
qn kv set list
```

**Make on-chain calls (no endpoint to provision):**

```sh
qn tooling-access enable --yes        # one-time; idempotent, admin role required
qn rpc call eth_blockNumber           # → "0x…" (default network)
qn rpc call eth_getBalance '["0xabc...", "latest"]'
qn rpc list-networks                  # available network keys for this endpoint
qn rpc call getSlot --network solana-mainnet
qn rpc call eth_blockNumber --endpoint-url https://my-endpoint.example/rpc
```

`qn rpc call` mints and refreshes the session JWT for you; the only one-time step
is enabling Tooling Access (or pass `--yes` to enable on first use). A custom
`--endpoint-url` (or `[rpc] endpoint_url` in config) bypasses that entirely.

**Pay per call with a crypto micropayment (no API key, no login):**

`--payment-asset`/`--max-amount` must match a real offer for the network (see
`qn rpc x402 supported-payments` / `qn rpc mpp supported-payments`); an
unfunded wallet or a mismatched asset/amount is refused with HTTP 400/402
before anything settles.

```sh
qn rpc x402 supported-networks                       # networks you can call (mpp: qn rpc mpp supported-networks)
qn rpc x402 supported-payments                       # payment options: network, token, address (mpp: qn rpc mpp supported-payments)
qn wallet generate --vm evm --name payer             # dedicated wallet; prints its address + a QR to fund
# → fund THAT address, then pick a lane:

# x402 on EVM (pays Base Sepolia USDC; --network, the chain you query, is
# independent of the chain you pay on):
qn rpc call eth_blockNumber --network ethereum-mainnet --x402 \
    --payment-wallet payer --payment-network base-sepolia \
    --payment-asset USDC --max-amount 1000

# MPP (pays Tempo testnet USDC); same EVM wallet, --receipt adds the settlement ref:
qn rpc call eth_blockNumber --network ethereum-mainnet --mpp --receipt \
    --payment-wallet payer --payment-network tempo-testnet \
    --payment-asset USDC --max-amount 1000

# x402 on Solana (devnet); needs an SVM wallet:
qn wallet generate --vm svm --name sol-payer
qn rpc call getSlot --network solana-devnet --x402 \
    --payment-wallet sol-payer --payment-network solana-devnet \
    --payment-asset USDC --max-amount 1000

# Store the parameters in config to keep calls short (never the raw key):
cat >> ~/.config/qn/config.toml <<'EOF'
[rpc.payment]
wallet          = "payer"                         # a stored wallet, or key_file = "<path>" (chmod 600)
max_amount      = "1000"                          # spend ceiling per call, base units
payment_network = "base-sepolia"                  # settlement chain: network name or CAIP-2 id
payment_asset   = "USDC"                          # symbol (resolved per network), or a raw address/mint
EOF
qn rpc call eth_blockNumber --network ethereum-mainnet --x402   # params now come from config
```

This moves real funds (even testnet tokens are real transfers) — use a
dedicated, minimally funded wallet. The spend ceiling bounds each call; there
is no built-in default.

## 8. Gotchas & safety rails

- Mutations are never retried; re-running a failed create can double-provision (§5).
- Paid `rpc call` moves real funds and never auto-retries; exit 3 means the
  payment may have settled — check the wallet before re-running (§3, §5). The
  CLI never prints the payment key; it comes only from a key file or a stored
  wallet (never an env var, never argv, never inline in config).
- No account-wide wipe command exists by design (§4).
- Piped output defaults to `json`; pass `-o toon` for the compact LLM form (§2).
- `--base-url` overrides the API host; it exists for testing.
- For *this* command, `-o yaml`/`-o toon`/`-o table` print Markdown (with a note on
  stderr); `-o json` produces the `{version, guide}` envelope.

## 9. More

- `qn --help`, and `--help` at every noun/verb level, document flags exhaustively.
- Docs: https://www.quicknode.com/docs
- This guide self-describes its version: it matches qn v{{VERSION}}.
