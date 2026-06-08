# Claude's working notes for `qn`

This file documents the patterns established while building `qn` so future work stays consistent. It assumes you've read the project [README](./README.md) for the user-facing surface.

## ⚠️ This is an open-source repo

Everything in this repository is — or will be — public. Treat every file, commit message, issue, and PR description as world-readable.

**Brand capitalization: always `Quicknode` (capital Q, lowercase n).** Never `QuickNode`, `quickNode`, or `QUICKNODE`. This applies to every surface — code, comments, commit messages, docs, generated workflow strings, formula descriptions, package metadata, error messages, anywhere the brand name appears.

- **Never commit secrets.** No API keys, tokens, account IDs, customer identifiers, internal hostnames, or non-public URLs. The CLI itself reads keys from env vars or `~/.config/qn/config.toml`; both live outside the repo. If you spot a leaked secret in a diff, stop and flag it before committing.
- **Never include internal Quicknode information** in code, comments, commit messages, fixtures, snapshots, or test data: no internal Slack/Linear/Jira links, no employee names, no internal team structure, no unreleased product/feature names, no non-public roadmap detail, no private infrastructure details. If something would be inappropriate to post on the public crates.io page, it doesn't belong here.
- **Test fixtures must use fake data.** Endpoint IDs like `ep-1`, wallets like `0xabc`, URLs like `https://hook.example.com`, emails like `alice@example.com`. Don't paste real responses from a real account.
- **No internal-only justifications in commit messages or comments.** "Fixes the bug from the Slack thread" leaks the existence of the Slack thread. State the technical reason instead: "Fixes incorrect URL path for `update_endpoint_status`."
- **External links are fine** for public docs (the SDK README on crates.io, the CLI guidelines at clig.dev, the Quicknode public docs site). Internal wikis, dashboards, or staging URLs are not.

When in doubt, ask before committing. The cost of pausing is low; the cost of a leak in git history is permanent.

## What this crate is

A Rust CLI that wraps the `quicknode-sdk` crate (v0.1). The SDK has four sub-clients (`admin`, `streams`, `webhooks`, `kvstore`); `qn` exposes every public method on each as a noun-verb subcommand. CLI behavior follows the [Command Line Interface Guidelines](https://clig.dev).

## Architecture

Single binary crate with a library target so integration tests can dispatch in-process.

```
src/
├── main.rs            # tokio::main, parse + dispatch + map CliError → exit code
├── lib.rs             # pub mod re-exports for tests
├── cli.rs             # clap derive Cli + top-level dispatch (run)
├── context.rs         # Ctx { sdk, out, global, key_source, config_path } + GlobalArgs
├── config.rs          # api-key resolution + ~/.config/qn/config.toml
├── confirm.rs         # destructive-action gating (mild / severe)
├── errors.rs          # CliError + exit-code mapping + SDK→user rendering
├── output.rs          # Render trait, OutputCtx, table helpers, JSON path
├── time_arg.rs        # --from/--to parsing (now | 1h | RFC-3339)
└── commands/          # one module per top-level noun
    ├── auth.rs        # the only one that doesn't build the SDK upfront
    ├── endpoint/      # large surface → submodule directory
    │   ├── mod.rs
    │   ├── render.rs  # pub(crate) — shared with metrics.rs
    │   ├── security.rs
    │   ├── ratelimit.rs
    │   └── tag.rs
    └── {team,usage,metrics,chain,billing,stream,webhook,kv}.rs
```

## Adding a new subcommand: the playbook

### 1. Inspect the SDK first

Before writing any CLI code, look at the real signatures, request structs, and response shapes. The README has outdated examples in places (e.g. it shows `SdkFullConfig::builder()` but the SDK actually exposes `SdkFullConfig::from_api_key(key)` and a `bon::Builder` derive). Trust the source, not the README.

```sh
ls ~/.cargo/registry/src/index.crates.io-*/quicknode-sdk-*/src/
grep -nE "pub( async)? fn " ~/.cargo/registry/src/index.crates.io-*/quicknode-sdk-*/src/admin/mod.rs
```

Things to verify for each new endpoint:

- **HTTP method and URL path** (e.g. `update_endpoint_status` is `PATCH /endpoints/{id}/status`, NOT POST — the README implied POST).
- **Response wrapper shape**: most admin responses are `{ data: Option<T>, error: Option<String> }`, but `UpdateEndpointStatusResponse::data` is `Option<String>`, and KV responses are wrapped via an internal `ApiResponse<T>` so the mock needs `{"data":{"value":"v"}}`, not `{"value":"v"}`.
- **Serde rename rules**: `ActivateWebhookParams` is `rename_all = "camelCase"` — the wire body is `{"startFrom":"latest"}`, not `start_from`. `SecurityOptionsUpdate.domain_masks` serializes as `domainMasks`. `BulkSetsParams` is camelCase too.
- **Required vs optional fields**: `UsageData.start_time/end_time` are required `i64`, so mocks must include them or decoding fails.
- **Field names** the docs got wrong: SDK uses `MethodRateLimiter.created` (not `created_at`); `EndpointDomainMask.domain` (not `domain_mask`); `EndpointReferrer.referrer: Option<String>`; `BulkTag.tag_id` (not `id`).

### 2. Map SDK signatures to the CLI

- **HTTP-noun → CLI-noun-verb**: `admin.get_endpoints` → `qn endpoint list`, `admin.show_endpoint` → `qn endpoint show <ID>`, `admin.update_endpoint_status(id, "paused")` → `qn endpoint pause <ID>` (split the verb out, since the user thinks "pause", not "update status to paused").
- **Aliases**: every list-style command gets `#[command(visible_alias = "ls")]`. Plural top-level nouns get one too (`#[command(visible_alias = "endpoints")]`).
- **Hyphenation**: clap kebab-cases enum variants by default. `RateLimit` → `rate-limit`. Test invocations must use the kebab form (`qn endpoint rate-limit method-create`, not `ratelimit`).
- **Negative numbers**: any `i64` flag that accepts `-1` (`--end`, etc.) needs `#[arg(long, allow_hyphen_values = true)]` or clap will read it as another flag.
- **Multi-value flags**: prefer repeatable `--method foo --method bar` (clap `Vec<String>` with `#[arg(long = "method")]`). Optionally also accept `--methods foo,bar` via a second field with `value_delimiter = ','`. The command body extends one into the other.
- **Large `Args` structs**: clippy's `large_enum_variant` will reject an enum variant whose payload is > ~200 bytes. Box it: `Create(Box<CreateArgs>)` (we hit this on `StreamCmd::Create` and `WebhookCmd::Create`).
- **Destructive operations**: route through `confirm::decide_without_prompt(Severity::Mild|Severe, ConfirmCfg)`. Mild = single `--yes` skips, TTY prompts otherwise. Severe = `--yes --yes` skips OR user types a literal word (`delete-all`). Non-TTY without enough `--yes` returns `CliError::NeedsConfirmation` (exit 5).
- **Args that span multiple subcommands**: for things like rate-limits where both account-level and method-level CRUD exists, use a single `RateLimitCmd` enum with `MethodList/MethodCreate/...` variants — don't fight clap by trying to nest a third level.

### 3. Module layout

- A simple command stays in `src/commands/<noun>.rs`.
- A complex one (endpoint has 10+ verbs plus nested security/rate-limit/tag subcommands) becomes `src/commands/<noun>/{mod.rs, render.rs, …}`. `render.rs` is `pub(crate)` so other modules (e.g. metrics.rs) can borrow `metric_series` instead of duplicating it.
- Every command module exposes exactly two things: `pub struct Args` (clap-derive) and `pub async fn run(args: Args, ctx: Ctx) -> Result<(), CliError>`. The dispatcher in `cli.rs` calls `run` with `Ctx::from_global(global)` — except `auth`, which takes `GlobalArgs` directly because it sometimes runs *without* a valid API key.

### 4. Building the request

```rust
let mut req = GetEndpointsRequest {
    limit: a.limit,
    offset: a.offset,
    ..Default::default()
};
if !a.networks.is_empty() {
    req.networks = Some(a.networks);  // SDK wants Option<Vec<_>>, empty vs missing matters
}
let resp = ctx.sdk.admin.get_endpoints(&req).await?;
```

- Convert empty `Vec` flags into `None` before sending — the SDK uses `Option<Vec<T>>` deliberately (empty is a valid filter value in the query string).
- The SDK's `SdkError` propagates via `?` into `CliError` through the `#[from] SdkError` impl. Don't try to handle SDK errors locally; let them bubble up to `errors::render`.
- The SDK has no builder for most request structs — construct them as field literals.

### 5. Rendering the response

`output.rs` defines:

```rust
pub trait Render: Serialize { fn render_table(...) -> io::Result<()>; }
pub fn emit<T: Render>(ctx: &OutputCtx, value: &T) -> Result<(), CliError>;
```

- For each response type, create a newtype wrapper inside the command module: `#[derive(Serialize)] struct EndpointsView(GetEndpointsResponse);` then `impl Render for EndpointsView`. This avoids orphan-rule conflicts on the foreign types and keeps JSON output identical to the SDK's wire format.
- Use `output::new_table()`, `output::opt_cell(&opt)` (renders `—` for `None`), `output::bool_cell(opt_bool)` (✓/✗/—), `output::write_table(w, &table)`.
- Most response data fields are `Option<DataStruct>`. Pattern: `let data = match &self.0.data { Some(d) => d, None => { writeln!(w, "(no data)")?; return Ok(()); } };` — never panic on a missing wrapper.
- State-changing commands write a confirmation line to stderr via `ctx.out.note("✓ Did the thing")`. The actual resource (if any) still goes to stdout via `emit`.
- For single-value responses (`EnabledCountResponse { total }`, `ListContainsItemResponse { exists }`), special-case `--json` and otherwise print the bare value: `println!("{}", resp.total)`. Don't render a 1-row table.

### 6. Wiring it into `cli.rs`

`cli.rs` is the single source of truth for the command surface. Add the new variant to `Command`, add the dispatch arm in `Cli::run`. Don't bypass — every command goes through `Ctx::from_global(global)?` so auth and base-url resolution stays uniform.

### 7. Tests

Per-command, write at least:

1. **Happy path** (correct path, correct method, correct body, valid response, exit 0). Use `body_json` for exact-match payloads; `body_partial_json` when the SDK sends extra fields and we only care about a few.
2. **Error path** (404 / 401 / 500 → mapped exit code 2 with the right user-facing message).
3. **Destructive-confirmation path** if applicable (no-yes returns 5, `--yes` returns 0).

Tests live in `tests/<sub_client>.rs` and use the `tests/common/mod.rs` harness:

```rust
mod common;
use common::run_qn;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path, body_json};
use serde_json::json;

#[tokio::test]
async fn my_command_does_the_thing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/endpoints/ep-1/some/path"))
        .and(body_json(json!({ "field": "value" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": "ok" })))
        .mount(&server).await;
    let out = run_qn(&server.uri(), &["endpoint", "do-thing", "ep-1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}
```

`run_qn` parses with `clap::Parser::try_parse_from`, builds an SDK pointed at the wiremock server via the hidden `--base-url` flag, and dispatches in-process. Fast, deterministic, no subprocess spawn.

Use a real subprocess (`assert_cmd::Command::cargo_bin("qn")`) ONLY when you need to verify the binary itself — argv parsing edge cases, exit codes the harness can't observe, stdout content. See `tests/cli_smoke.rs` and `list_endpoints_json_output_is_valid_json` in `tests/endpoint.rs`.

### 8. Snapshots

Table layout changes — anything affecting the human-facing render — should land via an insta snapshot diff. Run `cargo insta review` to inspect and accept. Snapshot files live under `tests/snapshots/`. To regenerate from scratch: `INSTA_UPDATE=always cargo test --test output_snapshots`.

## Verifying changes

```sh
cargo test                            # all 107+ tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --release                 # confirm no warnings in release
```

For TTY-specific behavior (color, prompting), run `./target/debug/qn ...` in a real shell — the test suite can't reliably simulate a TTY.

## Conventions to keep

- **Error messages**: actionable. `Error: not found.` is a stub. `Error: unauthorized. Check your API key with 'qn auth whoami'.` is right. The SDK error → user message map lives in `errors::render`; extend it for new failure modes instead of letting raw `SdkError` Display leak through.
- **Exit codes** (from `errors::exit_code_for`): 0 ok, 1 generic CLI error, 2 SDK Api error, 3 SDK Http error, 4 NoApiKey/BadConfig/ConfigWrite, 5 cancelled/needs-confirmation, 130 SIGINT. Don't invent new codes; map new error variants into the existing buckets.
- **No secrets in logs**: API keys never go to stdout. `auth whoami` redacts to `****<last4>`. Never `dbg!` or `println!` an `SdkFullConfig`.
- **`ctx.out.note`** for ✓ state-change confirmations on stderr (auto-suppressed by `--quiet`). The actual resource still goes to stdout via `emit` so pipelines see it.
- **Don't read `std::env` inside command bodies.** Resolve through `Ctx` / `GlobalArgs`. Tests need to set behavior without mutating process env (which races across parallel tests). `OutputCtx::detect_with` is the testable form of `OutputCtx::detect` for this reason.

## Anti-patterns we already hit and fixed

1. **Treating `SdkError` as a single error type** — it's an enum, and the SDK exposes a `http_kind()` helper that we use to distinguish timeout/connect/other. Match on variants in `errors::render`, don't `to_string()`.
2. **Trusting the README's URL/method paths** — verify against the SDK source (`grep 'base_url.join\|format!(' admin/mod.rs`). Several mocks failed initially because we guessed wrong (`/multichain/enable` vs `/enable_multichain`, `/security/options` vs `/security_options`).
3. **`Vec<i64>` + `Cell::new` rendering** — `comfy_table::Cell::new(value)` works for `i64`, `i32`, `&str`, `String`, `usize`. Don't reach for `format!` unless you have to.
4. **Forgetting `Option<Vec<…>>`** — endpoint security fields like `tokens` are `Option<Vec<EndpointToken>>`. `.len()` on `Option` is private — use `.as_deref().unwrap_or(&[]).len()`.
5. **Mutating env vars in `#[test]` fns** — Rust tests run in parallel and env is process-global. Refactor the code under test to take env values as parameters (see `OutputCtx::detect_with`).
6. **clap auto-kebab-case mismatches** — `RateLimitCmd::MethodCreate` becomes `method-create`. Test invocations must match.
7. **Negative integer args without `allow_hyphen_values`** — `--end -1` is parsed as the flag `-1` unless you opt in.

## Commit conventions

One commit per stage of the plan. Body explains the user-visible surface (`qn endpoint pause` etc.) and the test count. Don't add Claude/Anthropic trailers unless explicitly asked. Don't amend; create new commits even on hook failure.
