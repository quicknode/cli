# Implementation plan: x402 drawdown + MPP session

Source of truth for the UX design:
`~/.claude/plans/research-quicknode-gateway-payment-vast-babbage.md`.
This file tracks execution status only; do not re-litigate the 14 decisions.

Branch `x402_MPP`, one PR, both models, phased commits (x402 drawdown first,
then MPP session). Do NOT commit this file (public repo).

Pre-stage (done): popped stash and committed the per-request `--payment-*`
flag rename + asset-name resolution. Plan naming is now real.

## Stage 1: SDK x402 drawdown
**Goal**: SIWX auth (POST /auth), JWT seed/export, drawdown call (Bearer, no
signing), GET /credits, POST /drip, credit purchase via existing 402 signer.
New `PaymentScheme` variant(s) in `../sdk/crates/core/src/rpc/payment/mod.rs`.
**Success criteria**: SDK wiremock unit tests; per-request paths untouched.
**Status**: Complete (SDK commit e6416f4)

## Stage 2: CLI `qn rpc x402` noun
**Goal**: buy-credits/balance/drip in `src/commands/rpc/x402.rs`; JWT cache
(0600, wallet-address keyed); Mild gating w/ ceiling-naming prompt; next hints.
**Success criteria**: happy + error + both gating tests in tests/rpc_payment.rs.
**Status**: Complete (CLI commit 21ef1b1)

## Stage 3: CLI `--x402-drawdown` on call
**Goal**: ArgGroup member; auto re-auth; error mapping (empty credits, monthly
limit). context.md + README in same commits.
**Success criteria**: wiremock tests incl. expired-JWT re-auth; single-attempt.
**Status**: Complete (CLI commit 7b11106)

## Stage 4: SDK MPP session
**Goal**: escrow deposit tx (Tempo first), open/top-up/close/status, cumulative
EIP-712 voucher signer, `/session/:network` prefix via `host_base`.
**Success criteria**: SDK tests; charge-intent path untouched.
**Status**: Complete (SDK commit 6da9adc; byte-exact voucher+channelId vectors)

## Stage 5: CLI `qn rpc mpp` noun + `--mpp-session` on call
**Goal**: open/top-up/close/status in `src/commands/rpc/mpp.rs`; channel state
file + recovery-via-status; Mild gating incl. close; voucher call mode.
**Success criteria**: full test matrix; snapshot review for table output.
**Status**: Complete (SDK 06b5bc4; CLI ab0c188)

## Stage 6: README Micropayments refactor + docs polish
**Goal**: one Micropayments section, four zero-to-call walkthroughs + shared
Get-a-wallet preamble; call --help examples; deferral notes.
**Success criteria**: all four walkthroughs copy-pasteable; full verify clean.
**Status**: In Progress
