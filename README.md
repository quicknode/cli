# qn — Quicknode CLI

`qn` is a command-line interface for Quicknode, built around noun-verb commands that read naturally for both humans and agents. Manage endpoints, streams, webhooks, the KV store, teams, usage, and billing, with output in multiple formats for easy reading or scripting.

```
$ qn endpoint list
ID    LABEL       STATUS  CHAIN/NETWORK     TYPE       MULTI
ep-1  production  active  ethereum/mainnet  shared     yes
ep-2  —           paused  solana/mainnet    dedicated  no
showing 1–2 of 2

$ qn endpoint list --wide
ID    LABEL       STATUS  CHAIN/NETWORK     TYPE       MULTI  HTTP                    WSS
ep-1  production  active  ethereum/mainnet  shared     yes    https://ep-1.example    —
ep-2  —           paused  solana/mainnet    dedicated  no     https://ep-2.example    —
showing 1–2 of 2

# Piped / non-TTY output defaults to JSON
$ qn endpoint list | cat
{
  "data": [
    {
      "id": "ep-1",
      "name": "ep-1",
      "label": "production",
      "status": "active",
      "chain": "ethereum",
      "network": "mainnet",
      "is_dedicated": false,
      "is_flat_rate": false,
      "http_url": "https://ep-1.example",
      "wss_url": null,
      "tags": ["prod", "eu"],
      "is_multichain": false
    },
    {
      "id": "ep-2",
      "name": "ep-2",
      "label": null,
      "status": "paused",
      "chain": "solana",
      "network": "mainnet",
      "is_dedicated": true,
      "is_flat_rate": false,
      "http_url": "https://ep-2.example",
      "wss_url": null,
      "tags": [],
      "is_multichain": false
    }
  ],
  "pagination": { "total": 2, "limit": 20, "offset": 0 },
  "error": null
}
```

## Installation

Pick the recommended path for your platform. Other channels are listed under [Alternatives](#alternatives).

### Homebrew (macOS)

```sh
brew install quicknode/tap/qn
```

Homebrew installs shell completions automatically — open a new shell after
install and `qn <TAB>` works. zsh users may have one extra requirement: zsh only
autoloads a completion when its directory is on `$fpath` before `compinit` runs
at shell startup. If `qn <TAB>` lists files instead of subcommands, the Homebrew
completions directory is missing from `$fpath` — see the
[zsh completion-system manual](https://zsh.sourceforge.io/Doc/Release/Completion-System.html).

### Scoop (Windows)

```powershell
scoop bucket add quicknode https://github.com/quicknode/scoop-bucket
scoop install quicknode/qn
```

### `.deb` (Debian, Ubuntu)

Each GitHub release attaches a `.deb` per architecture. These canonical URLs always point at the latest release — check your architecture with `dpkg --print-architecture` and pick the matching one:

```sh
# amd64 (Intel/AMD)
curl -LO https://github.com/quicknode/cli/releases/latest/download/qn_amd64.deb
sudo apt install ./qn_amd64.deb

# arm64
curl -LO https://github.com/quicknode/cli/releases/latest/download/qn_arm64.deb
sudo apt install ./qn_arm64.deb
```

Versioned files (`qn_<VERSION>_amd64.deb`) are also attached to each release for pinning.

### Arch Linux (AUR)

```sh
yay -S qn-bin   # or any other AUR helper
```

### Fedora, EPEL (COPR)

```sh
sudo dnf copr enable quicknode/qn
sudo dnf install qn
```

### Docker (GHCR)

```sh
docker pull ghcr.io/quicknode/qn:latest
docker run --rm ghcr.io/quicknode/qn:latest --help
```

### Alternatives

<details>
<summary>crates.io, from source, prebuilt binaries</summary>

**crates.io:**

```sh
cargo install quicknode-cli
```

The crate name is `quicknode-cli` but the installed binary is `qn`.

**From source:**

```sh
git clone git@github.com:quicknode/cli.git && cd cli
cargo install --path .
```

**Prebuilt binaries:** every GitHub release attaches per-platform archives — see the [latest release page](https://github.com/quicknode/cli/releases/latest).

</details>

## Authentication

You will need a Quicknode API key to get started. Once you have that, you can run `qn auth login`

`qn` resolves your API key from the first source that matches:

1. `--api-key <KEY>` flag
2. The config file: the `--config-file <PATH>` flag if given, otherwise
   `~/.config/qn/config.toml` — or `$XDG_CONFIG_HOME/qn/config.toml` if that
   env var is set. The same layout applies on Windows:
   `%USERPROFILE%\.config\qn\config.toml`. Managed by `qn auth login`.

If no source matches, `qn` exits with code 4 and tells you to run
`qn auth login`.

```sh
qn auth login      # prompts for the key, writes it to ~/.config/qn/config.toml
qn auth whoami     # confirms the key works against the live API
qn auth logout     # removes the saved key
```

## Output

Pick a format with `--format <FMT>` (alias `-o <FMT>`):

| `--format` | Best for |
| --- | --- |
| `table` | Humans on a TTY. Pretty UTF-8 tables with optional color. Default when stdout is a terminal. |
| `json`  | Scripts and pipelines (`jq`, `gron`, …). Default when stdout is **not** a terminal (piped / agent invocations). |
| `yaml`  | Same shape as JSON, easier to skim by eye. |
| `md`    | GitHub-flavored markdown — paste into PRs, issues, docs. |
| `toon`  | [Token-Oriented Object Notation](https://github.com/toon-format/toon-rust) — compact serialization optimized for LLM prompts. |

Other output flags:

- **`-w` / `--wide`:** add extra columns to `table` and `md` output (e.g. HTTP/WSS URLs in `endpoint list`). Mirrors `kubectl get -o wide`. Doesn't affect `json`/`yaml`/`toon`, which always include everything.
- **`--no-color`:** plain ASCII (also honored: `NO_COLOR` env var, `TERM=dumb`, non-TTY stdout, any non-`table` format).
- **`--quiet`:** suppress state-change notes on stderr.
- **`--verbose`:** include API error bodies and other detail.

You can also set defaults in `~/.config/qn/config.toml`:

```toml
[output]
format = "yaml"   # default --format value
wide = true       # always show extra columns in table/md output
```

CLI flags win over config values. Built-in defaults: `format = "table"` when stdout is a TTY, `"json"` otherwise; `wide = false`.

`qn` follows the [Command Line Interface Guidelines](https://clig.dev/): data on stdout, diagnostics on stderr, meaningful exit codes (0 success, 2 API error, 3 network error, 4 auth/config, 5 needs confirmation), and a documented `-h`/`--help` at every subcommand level.

## Example usage

### Endpoints

```sh
qn endpoint list -o json | jq '.data[].id'
qn endpoint create --chain ethereum --network mainnet
qn endpoint show ep-1234
qn endpoint pause ep-1234
qn endpoint logs ep-1234 --from 1h --to now --limit 50
qn endpoint metrics ep-1234 --metric method_calls_over_time --period day
qn endpoint security set-options ep-1234 --tokens enabled --jwts disabled
qn endpoint rate-limit set ep-1234 --rps 100 --rpm 5000
qn endpoint archive ep-1234 --yes
```

### Streams

```sh
qn stream list --limit 20
qn stream create \
  --name my-stream \
  --network ethereum-mainnet \
  --dataset block \
  --start 24691804 --end=-1 \
  --region usa-east \
  --webhook https://webhook.site/abc \
  --batch-size 1
qn stream activate s-1234
qn stream test-filter \
  --network ethereum-mainnet \
  --dataset block \
  --block 17811625 \
  --filter-file filter.js
qn stream delete s-1234 --yes
```

### Webhooks

```sh
qn webhook list
qn webhook create \
  --name "wallet alerts" \
  --network ethereum-mainnet \
  --url https://webhook.site/abc \
  --template evm-wallet \
  --wallet 0xa0b8...
qn webhook create \
  --name "uniswap events" \
  --network ethereum-mainnet \
  --url https://webhook.site/xyz \
  --template evm-contract-events \
  --contract 0x88e6...
qn webhook activate wh-1 --start-from latest
qn webhook pause wh-1
```

### KV store

```sh
qn kv set put my-key my-value
echo "value from stdin" | qn kv set put my-key -
qn kv set get my-key
qn kv set ls

qn kv list create allowlist 0xabc 0xdef
qn kv list append allowlist 0x123
qn kv list contains allowlist 0xabc
qn kv list get allowlist
```

### SQL

```sh
# Run a query inline, from a file, or from stdin (--file -)
qn sql query "SELECT action_type, user FROM hyperliquid_system_actions ORDER BY block_time DESC LIMIT 3" --cluster-id hyperliquid-core-mainnet
qn sql query --file query.sql --cluster-id hyperliquid-core-mainnet
cat query.sql | qn sql query --file - --cluster-id hyperliquid-core-mainnet

# Pipe rows into jq (stats print to stderr, so stdout stays clean)
qn sql query "SELECT 1" --cluster-id hyperliquid-core-mainnet -o json | jq '.data'

# Inspect a cluster's tables, columns, and types
qn sql schema hyperliquid-core-mainnet
```

Queries are read-only (SELECT) and capped at 1000 rows per request; page through
larger result sets with `LIMIT`/`OFFSET` in the SQL.

### On-chain RPC

Make JSON-RPC calls with no endpoint to provision. `qn rpc call` mints and
refreshes a short-lived session JWT automatically; the only one-time step is
enabling Tooling Access (or pass `--yes` to enable on first use).

```sh
qn tooling-access enable               # one-time; idempotent, requires an admin role
qn tooling-access status

qn rpc call eth_blockNumber
qn rpc call eth_getBalance '["0xabc...", "latest"]'
qn rpc call eth_call '{"to":"0x..."}'
qn rpc call eth_call --params-file params.json   # read params from a file (-f)
echo '[...]' | qn rpc call eth_call -             # read params from stdin

qn rpc call eth_blockNumber --yes      # auto-enable Tooling Access if needed

# Multichain: the endpoint serves many chains. Target one by its network key.
qn rpc list-networks                   # list available network keys (alias: ls)
qn rpc call getSlot --network solana-mainnet
qn rpc call eth_chainId --network polygon

# Custom endpoint: send the call to a fully-formed HTTP URL instead of Tooling
# Access. The URL is self-authenticating (no session token is minted or sent).
qn rpc call eth_blockNumber --endpoint-url https://my-endpoint.example/rpc
```

Set a default custom endpoint in `~/.config/qn/config.toml` so every `qn rpc call`
uses it without the flag (a per-call `--endpoint-url` still overrides it):

```toml
[rpc]
endpoint_url = "https://my-endpoint.example/rpc"
```

`--endpoint-url` and `--network` are mutually exclusive: a custom URL is not
multichain-routed.

The network map is cached in `~/.config/qn/networks.toml` (per endpoint, 24h TTL),
so `--network` calls reuse it without re-fetching. Network keys are the endpoint's
own `multichain_urls` keys (note these can differ from chain slugs, e.g. `polygon`
not `matic`); `qn rpc list-networks` shows the exact set.

The session token is cached under `~/.config/qn/tokens.toml` (0600), scoped to the
API key, so subsequent calls skip the mint round trip while it's valid. Results are
schemaless JSON; `-o json|yaml|toon` controls the format (`table`/`md` fall back to
JSON).

#### Pay per call with a crypto micropayment

`--x402` (EVM or Solana stablecoin) or `--mpp` (Tempo) pays for the call per
request instead of using an account API key — no login and no Tooling Access
needed. **This moves real funds** (testnet tokens are still real transfers):
use a dedicated, minimally funded wallet.

The quickest path: see what's payable, generate a wallet, fund it, then pay by
name.

```sh
qn rpc pay-networks                                   # payable networks + the x402 asset per network
qn rpc wallet generate --chain evm --name payer       # new wallet; prints its address + a QR to fund it
# → send funds to that address, then:
qn rpc call eth_blockNumber --network base-sepolia --x402 \
    --payment-wallet payer \
    --pay-network base-sepolia \
    --asset 0x036CbD53842c5426634e7929541eC2318f3dCF7e \
    --max-amount 10000

# MPP settles on Tempo and returns a settlement receipt with --receipt:
qn rpc call eth_blockNumber --network tempo-testnet --mpp --receipt ...
```

You can also point `--payment-key-file <PATH>` at a key file you manage
yourself instead of a stored wallet.

- `--network` is required and names the chain you *query*, as the payment
  gateway's path slug (independent of `--pay-network`, the chain the payment
  *settles* on).
- `--pay-network` takes a Quicknode network name (`base-sepolia`,
  `solana-devnet`, `tempo-testnet`, ...) or a raw CAIP-2 id (`eip155:84532`,
  `solana:EtWTRA...`). Anything containing a `:` is passed through verbatim,
  so any `eip155:<chain-id>` works even without a named entry.
- The private key resolves from `--payment-key-file <PATH>` (`-` for stdin),
  then `--payment-wallet <NAME>` (a stored wallet), then `key_file`, then
  `wallet` in config. It always comes from a file or a stored wallet — never
  an environment variable, never a flag value, never inline in config, and
  never printed.
- `--max-amount` is the per-call spend ceiling in integer base units of the
  asset (e.g. `10000` = 0.01 USDC). There is no built-in default; offers above
  the ceiling are refused before anything is signed.
- `--receipt` wraps stdout as `{"result": ..., "payment_receipt": ...}`. On
  MPP the receipt is an object (`method`, `status`, `timestamp`, and
  `reference` — the settlement transaction hash); on x402 it is `null`.
  Without it, paid output is shaped exactly like unpaid output.
- Paid calls **never auto-retry** (`--retries` does not apply). Exit code 2
  means the gateway refused and nothing settled; exit 3 means the outcome is
  unknown — the payment was submitted and may have settled; check the wallet
  before re-running.
- x402/Solana at volume: pass `--svm-rpc-url <URL>`; the default public Solana
  RPC rate-limits aggressively.

Store the parameters once in `~/.config/qn/config.toml` and the invocation
shrinks to the scheme flag — config supplies values but never activates
payment by itself:

```toml
[rpc.payment]
wallet      = "payer"                             # a stored wallet name (or key_file = "<path>")
max_amount  = "10000"
pay_network = "base-sepolia"                      # network name or CAIP-2 id
asset       = "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
```

```sh
qn rpc call eth_blockNumber --network base-sepolia --x402
```

##### Managing payment wallets

`qn rpc wallet` keeps dedicated payment wallets under
`~/.config/qn/wallets/` (raw key at `0600`; the key is stored unencrypted, so
treat each as a dedicated, minimally funded hot wallet):

```sh
qn rpc wallet generate --chain evm --name payer   # evm also covers MPP/Tempo; svm for x402/Solana
qn rpc wallet list                                # names, chain, address (never the key)
qn rpc wallet show payer                          # address to stdout, plus a QR to fund it on a terminal
qn rpc wallet rm payer                            # gated: --yes to confirm; the key is unrecoverable
```

`qn rpc wallet show payer` prints only the bare address to stdout (the QR and
hint go to stderr), so `qn rpc wallet show payer` in a pipe yields just the
address.

##### Discovering payable networks

`qn rpc pay-networks` (alias `pay-nets`) lists the networks the paid lane can
use, read from the gateways' public discovery endpoints (no API key). A listed
network is a valid `--network`; the x402 asset column is a ready `--asset`
value. The list is cached in `~/.config/qn/pay-networks.toml` (24h TTL).

### Other

```sh
qn usage summary --from 7d
qn usage by-endpoint --from 30d -o yaml
qn metrics account --period day --metric credits_over_time
qn chain list
qn chain credits ethereum
qn billing invoices
qn endpoint bulk pause ep-1 ep-2 ep-3
qn endpoint tag list
qn team list
```

## Shell completions

When installing qn through a package manager, it's possible that no additional
shell configuration is necessary — Homebrew (see above) and distro packages
place the script for you. To set up completions manually, follow the
instructions below (`qn completions --help` prints the same). Exact config file
locations may vary by system; restart your shell before testing.

### bash

Install `bash-completion` with your package manager, then add to `~/.bashrc`:

```sh
eval "$(qn completions bash)"
```

### zsh

Homebrew already creates this `_qn` file for you on `brew install`. To set it up
manually, generate the script into a directory on your `$fpath` (Apple Silicon
shown; Intel brew uses `/usr/local/share/zsh/site-functions`):

```sh
qn completions zsh > /opt/homebrew/share/zsh/site-functions/_qn
```

Ensure that the following is present in your `~/.zshrc`:

```sh
autoload -U compinit
compinit
```

See the [zsh completion-system manual](https://zsh.sourceforge.io/Doc/Release/Completion-System.html) for details.

### fish

```sh
qn completions fish > ~/.config/fish/completions/qn.fish
```

### PowerShell

Add this line to your profile script (`$PROFILE`):

```powershell
qn completions powershell | Out-String | Invoke-Expression
```

Or append the generated script so it loads each session:

```powershell
qn completions powershell >> $PROFILE
```

## Configuration via environment

The conventional variables are honored: `NO_COLOR` and `TERM=dumb` disable color,
and `XDG_CONFIG_HOME`/`HOME` (`USERPROFILE` on Windows) locate the default
config file. The CLI hands the key to the Quicknode SDK explicitly; it does
not read the SDK's `QN_SDK__*` environment namespace.

The hidden `--base-url <URL>` flag overrides the API host for all four
sub-clients at once (used for integration tests and on-prem mirrors).

## Confirmations

Destructive commands (`delete`, `archive`, `bulk pause`, token revocation,
removing a rate-limit override, …) prompt before acting, and the prompt states
what will happen ("Pause 3 endpoint(s)? They will stop serving requests").
Pass `--yes`/`-y` to skip the prompt. In scripts and CI (no TTY), a gated
command without `--yes` exits with code 5 **before** any request is sent.

The CLI deliberately has no account-wide wipe commands (no `delete-all`);
operations with that blast radius belong behind the API, not a one-liner.

## Retries

Read-only commands (`list`, `show`, `logs`, `metrics`, `usage`, …) retry
transient failures — HTTP 429, 500, 502, 503, 504, timeouts, and connection
errors — with exponential backoff and full jitter. The default is 3 retries;
tune it with the global `--retries <N>` flag (`--retries 0` disables).
`stream test-filter` retries too: it sends a POST, but only evaluates a
filter against historical data and changes nothing.

Commands that modify resources (`create`, `update`, `delete`, `pause`, …)
**never** retry automatically: a retried create could provision twice. If a
mutation fails with a transient error, check whether it took effect before
re-running it.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | CLI error (usage/bad argument, IO, decode) |
| 2 | API error (server returned 4xx/5xx) |
| 3 | Network failure (timeout, connect, transport) |
| 4 | Missing or invalid API key / config |
| 5 | Operation needs confirmation (pass `--yes`) |
| 130 | Interrupted (SIGINT) |

## License

MIT
