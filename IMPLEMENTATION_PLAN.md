## Stage 1: SDK SIWS support
**Goal**: Authenticate Solana x402 drawdown sessions and settle Solana credit offers.
**Success criteria**: SDK tests cover SIWS construction, Base58 signatures, authentication, and Solana credit settlement.
**Status**: Complete

## Stage 2: Local SDK integration
**Goal**: Run the CLI against the local SDK implementation.
**Success criteria**: Cargo resolves `quicknode-sdk` from `../sdk/crates/core` with all payment features enabled.
**Status**: Complete

## Stage 3: CLI behavior and documentation
**Goal**: Expose Solana drawdown through CLI help, examples, and command handling.
**Success criteria**: Solana buy-credit and drawdown integration tests pass without changing EVM behavior.
**Status**: Complete

## Stage 4: Verification
**Goal**: Validate both repositories with formatting, tests, clippy, and builds.
**Success criteria**: Required checks pass with no new warnings.
**Status**: Complete

## Stage 5: Faucet compatibility and Arc Testnet
**Goal**: Support both asynchronous Circle transfer responses and direct Arc Testnet
transaction responses from `qn rpc x402 drip`.
**Success criteria**: Arc network and USDC resolution work, both response shapes
render correctly, and the CLI passes all verification checks against the local SDK.
**Status**: Complete
