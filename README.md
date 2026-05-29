# qn — Quicknode CLI

`qn` is a command-line interface for the [Quicknode SDK](https://crates.io/crates/quicknode-sdk). It exposes the full surface of the SDK — endpoints, streams, webhooks, KV store, teams, usage, metrics, billing — as a noun-verb CLI that's friendly to both humans and scripts.

```
$ qn endpoint list
┌─────────┬─────────┬────────┬───────────────────┬───────┬───────┐
│ ID      │ LABEL   │ STATUS │ CHAIN/NETWORK     │ TYPE  │ MULTI │
╞═════════╪═════════╪════════╪═══════════════════╪═══════╪═══════╡
│ ep-1234 │ prod    │ active │ ethereum/mainnet  │ shared│ no    │
└─────────┴─────────┴────────┴───────────────────┴───────┴───────┘
```

## Installation

```sh
cargo install qn
```

Or build from source:

```sh
git clone https://github.com/quicknode/qn && cd qn
cargo install --path .
```

## Authentication

`qn` resolves your API key from the first source that matches:

1. `--api-key <KEY>` flag
2. `QN_CLI__API_KEY` environment variable
3. `~/.config/qn/config.toml` (managed by `qn auth login`)

If none match, `qn` exits with code 4 and tells you to run `qn auth login`.
Regular commands never prompt — only `qn auth login` does. This keeps scripts
and CI deterministic.

```sh
qn auth login      # prompts for the key, writes it to ~/.config/qn/config.toml
qn auth whoami     # confirms the key works against the live API
qn auth logout     # removes the saved key
```

## Output

- **Default (TTY):** pretty ASCII tables with colors.
- **`--json`:** structured JSON — use this in scripts and pipelines.
- **`--no-color`:** plain ASCII (also honored: `NO_COLOR` env var, `TERM=dumb`, non-TTY stdout).
- **`--quiet`:** suppress state-change notes on stderr.
- **`--verbose`:** include API error bodies and other detail.

`qn` follows the [Command Line Interface Guidelines](https://clig.dev/): data on stdout, diagnostics on stderr, meaningful exit codes (0 success, 2 API error, 3 network error, 4 auth/config, 5 needs confirmation), and a documented `-h`/`--help` at every subcommand level.

## Example usage

### Endpoints

```sh
qn endpoint list --json | jq '.data[].id'
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

### Other

```sh
qn usage summary --from 7d
qn usage by-endpoint --from 30d --json
qn metrics account --period day --metric credits_over_time
qn chain list
qn billing invoices
qn bulk status --status paused ep-1 ep-2 ep-3
qn tag list
qn team list
```

## Shell completions

```sh
qn completions zsh  > ~/.zfunc/_qn        # zsh
qn completions bash > /etc/bash_completion.d/qn  # bash
qn completions fish > ~/.config/fish/completions/qn.fish
qn completions powershell > qn.ps1
```

## Configuration via environment

| Variable | Description |
|---|---|
| `QN_CLI__API_KEY` | Your Quicknode API key |

`qn` deliberately uses its own `QN_CLI__` namespace so the CLI's env vars don't
collide with — or silently leak into — direct use of the underlying SDK. The
CLI hands the key to the SDK explicitly; it does not read the SDK's
`QN_SDK__*` environment namespace.

The hidden `--base-url <URL>` flag overrides the API host for all four
sub-clients at once (used for integration tests and on-prem mirrors).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | CLI error (bad argument, IO, decode) |
| 2 | API error (server returned 4xx/5xx) |
| 3 | Network failure (timeout, connect, transport) |
| 4 | Missing or invalid API key / config |
| 5 | Operation needs confirmation (pass `--yes` or `--yes --yes`) |
| 130 | Interrupted (SIGINT) |

## License

MIT
