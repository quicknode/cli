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

# LLM-optimized TOON format (non-TTY default)
$ qn endpoint list | cat
data[2]{id,name,label,status,chain,network,is_dedicated,is_flat_rate,http_url,wss_url,tags,is_multichain}:
  "ep-1","ep-1","production",active,ethereum,mainnet,false,false,"https://ep-1.example",null,"prod, eu",false
  "ep-2","ep-2",null,paused,solana,mainnet,true,false,"https://ep-2.example",null,"",false
pagination:
  total: 2
  limit: 20
  offset: 0
error: null
```

## Installation

### From crates.io

```sh
cargo install quicknode-cli
```

The crate name is `quicknode-cli` but the installed binary is `qn`.

### Homebrew (macOS, Linux)

```sh
brew install quicknode/tap/qn
```

### From source

```sh
git clone git@github.com:quicknode/cli.git && cd cli
cargo install --path .
```

## Authentication

You will need a Quicknode API key to get started. Once you have that, you can run `qn auth login`

`qn` resolves your API key from the first source that matches:

1. `--api-key <KEY>` flag
2. The config file: the `--config-file <PATH>` flag if given, otherwise
   `~/.config/qn/config.toml` — or `$XDG_CONFIG_HOME/qn/config.toml` if that
   env var is set. Managed by `qn auth login`.

There is deliberately **no environment-variable key source**: a key left
exported in a shell is invisible state that outlives the session it was set
for, and makes it far too easy to run a destructive command against the wrong
account. For CI, write a config file and point `--config-file` at it (or pass
`--api-key` from your secret store).

If no source matches, `qn` exits with code 4 and tells you to run
`qn auth login`. Regular commands never prompt — only `qn auth login` does.
This keeps scripts and CI deterministic.

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
| `json`  | Scripts and pipelines (`jq`, `gron`, …). |
| `yaml`  | Same shape as JSON, easier to skim by eye. |
| `md`    | GitHub-flavored markdown — paste into PRs, issues, docs. |
| `toon`  | [Token-Oriented Object Notation](https://github.com/toon-format/toon-rust) — compact serialization optimized for LLM prompts. Default when stdout is **not** a terminal (piped / agent invocations). |

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

CLI flags win over config values. Built-in defaults: `format = "table"` when stdout is a TTY, `"toon"` otherwise; `wide = false`.

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

### Other

```sh
qn usage summary --from 7d
qn usage by-endpoint --from 30d -o yaml
qn metrics account --period day --metric credits_over_time
qn chain list
qn billing invoices
qn endpoint bulk pause ep-1 ep-2 ep-3
qn endpoint tag list
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

`qn` reads no API credentials from the environment (see
[Authentication](#authentication) for why). It honors the conventional
output-related variables only: `NO_COLOR` and `TERM=dumb` disable color. The
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
