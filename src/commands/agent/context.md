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

Config file location:

- Linux/macOS: `$XDG_CONFIG_HOME/qn/config.toml`, else `~/.config/qn/config.toml`.
- Windows: `%USERPROFILE%\.config\qn\config.toml`.

Verify the resolved key against the API: `qn auth whoami` (prints the key redacted
to `****<last4>` and confirms it works). `qn auth status` does the same without the
network call.

## 2. Output contract

- Default format is `table` on a TTY and **`toon`** when stdout is not a TTY (piped).
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

## 6. Command catalog

Top-level nouns (plurals like `endpoints`/`streams` and `ls` are accepted aliases):

- `auth` — login, logout, whoami, status
- `endpoint` — list, show, create, update, archive, pause, resume, urls, logs,
  log-details, metrics, enable-multichain, disable-multichain; nested:
  `tag`, `security`, `rate-limit`, `bulk`
- `team` — list, create, show, delete, endpoints, set-endpoints; nested: `member`
- `usage` — summary, by-endpoint, by-method, by-chain, by-tag
- `metrics` — account, endpoint
- `chain` — list
- `billing` — invoices, payments
- `stream` — list, show, create, update, delete, activate, pause, test-filter,
  enabled-count
- `webhook` — list, show, create, update, update-template, delete, activate, pause,
  enabled-count
- `kv` — `set` (put, get, list, delete, bulk) and `list` (list, get, create, append,
  contains, remove-item, update, delete)

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
  --url https://hook.example.com --template evm-wallet \
  --wallet 0xabc...                                      # → id
qn webhook show <id>                                     # inspect before activating
qn webhook activate <id>
```

**KV put / get / list:**

```sh
qn kv set put my-key my-value
qn kv set get my-key
qn kv set list
```

## 8. Gotchas & safety rails

- Mutations are never retried; re-running a failed create can double-provision (§5).
- No account-wide wipe command exists by design (§4).
- Piped output defaults to `toon`, not `json` (§2).
- `--base-url` overrides the API host; it exists for testing.
- For *this* command, `-o yaml`/`-o toon`/`-o table` print Markdown (with a note on
  stderr); `-o json` produces the `{version, guide}` envelope.

## 9. More

- `qn --help`, and `--help` at every noun/verb level, document flags exhaustively.
- Docs: https://www.quicknode.com/docs
- This guide self-describes its version: it matches qn v{{VERSION}}.
