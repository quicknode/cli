//! Human-readable names for `--pay-network`.
//!
//! The SDK's `PaymentConfig.pay_network` is a canonical CAIP-2 id — that is
//! the form the x402/MPP offers are matched against. This module is the CLI's
//! presentation layer on top: it accepts the same Quicknode network-name
//! vocabulary as `--network` (e.g. `base-sepolia`) and resolves it to CAIP-2
//! before the value reaches the SDK. Anything containing a `:` is treated as
//! an already-canonical CAIP-2 id and passed through verbatim, so every chain
//! is reachable even when it has no entry in the table.
//!
//! EVM chain ids are verified against the public EVM chain registry
//! (chainid.network, the dataset behind chainlist.org). Solana ids are the
//! standard CAIP-2 genesis-hash prefixes. Names whose chain id could not be
//! confirmed against a public source are deliberately absent — a wrong id
//! could match a payment offer on the wrong chain, while a missing name just
//! errors with the CAIP-2 escape hatch.

use crate::errors::CliError;

/// Quicknode network name → CAIP-2 pay-network id. Sorted by name (binary
/// searched); a unit test enforces order and uniqueness.
const PAY_NETWORKS: &[(&str, &str)] = &[
    ("0g-galileo", "eip155:16601"),
    ("0g-mainnet", "eip155:16661"),
    ("abstract-mainnet", "eip155:2741"),
    ("abstract-testnet", "eip155:11124"),
    ("arbitrum-mainnet", "eip155:42161"),
    ("arbitrum-sepolia", "eip155:421614"),
    ("ault-mainnet", "eip155:904"),
    ("ault-testnet", "eip155:10904"),
    ("avalanche-mainnet", "eip155:43114"),
    ("avalanche-testnet", "eip155:43113"),
    ("b3-mainnet", "eip155:8333"),
    ("base-mainnet", "eip155:8453"),
    ("base-sepolia", "eip155:84532"),
    ("bera-bepolia", "eip155:80069"),
    ("bera-mainnet", "eip155:80094"),
    ("blast-mainnet", "eip155:81457"),
    ("blast-sepolia", "eip155:168587773"),
    ("bsc", "eip155:56"),
    ("bsc-testnet", "eip155:97"),
    ("celo-mainnet", "eip155:42220"),
    ("cyber-mainnet", "eip155:7560"),
    ("ethereum-hoodi", "eip155:560048"),
    ("ethereum-mainnet", "eip155:1"),
    ("ethereum-sepolia", "eip155:11155111"),
    ("fantom", "eip155:250"),
    ("flare-coston2", "eip155:114"),
    ("flare-mainnet", "eip155:14"),
    ("fluent-mainnet", "eip155:25363"),
    ("fraxtal-mainnet", "eip155:252"),
    ("gravity-alpham", "eip155:1625"),
    ("hedera-mainnet", "eip155:295"),
    ("hedera-testnet", "eip155:296"),
    ("hemi-mainnet", "eip155:43111"),
    ("hemi-testnet", "eip155:743111"),
    // 999 is HyperEVM per Hyperliquid's docs; the chain registry still lists
    // it under a stale earlier registration.
    ("hype-mainnet", "eip155:999"),
    ("hype-testnet", "eip155:998"),
    ("injective-mainnet", "eip155:1776"),
    ("injective-testnet", "eip155:1439"),
    ("ink-mainnet", "eip155:57073"),
    ("ink-sepolia", "eip155:763373"),
    ("joc-mainnet", "eip155:81"),
    ("kaia-kairos", "eip155:1001"),
    ("kaia-mainnet", "eip155:8217"),
    ("katana-mainnet", "eip155:747474"),
    ("linea-mainnet", "eip155:59144"),
    ("lisk-mainnet", "eip155:1135"),
    ("mantle-mainnet", "eip155:5000"),
    ("mantle-sepolia", "eip155:5003"),
    ("megaeth-mainnet", "eip155:4326"),
    ("moca-testnet", "eip155:5151"),
    ("mode-mainnet", "eip155:34443"),
    ("monad-mainnet", "eip155:143"),
    ("monad-testnet", "eip155:10143"),
    ("morph-mainnet", "eip155:2818"),
    ("nova-mainnet", "eip155:42170"),
    ("optimism", "eip155:10"),
    ("optimism-sepolia", "eip155:11155420"),
    ("peaq-mainnet", "eip155:3338"),
    ("plasma-mainnet", "eip155:9745"),
    ("plasma-testnet", "eip155:9746"),
    ("polygon", "eip155:137"),
    ("polygon-amoy", "eip155:80002"),
    ("robinhood-mainnet", "eip155:4663"),
    ("robinhood-testnet", "eip155:46630"),
    ("sahara-testnet", "eip155:313313"),
    ("scroll-mainnet", "eip155:534352"),
    ("scroll-testnet", "eip155:534351"),
    ("sei-atlantic", "eip155:1328"),
    ("sei-pacific", "eip155:1329"),
    ("solana-devnet", "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"),
    ("solana-mainnet", "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"),
    ("solana-testnet", "solana:4uhcVJyU9pJkvQyS88uRDiswHXSCkY3z"),
    ("soneium-mainnet", "eip155:1868"),
    ("sonic-mainnet", "eip155:146"),
    ("story-aeneid", "eip155:1315"),
    ("story-mainnet", "eip155:1514"),
    ("tempo-mainnet", "eip155:4217"),
    ("tempo-testnet", "eip155:42431"),
    ("unichain-mainnet", "eip155:130"),
    ("unichain-sepolia", "eip155:1301"),
    ("vana-mainnet", "eip155:1480"),
    ("vana-moksha", "eip155:14800"),
    ("worldchain-mainnet", "eip155:480"),
    ("worldchain-sepolia", "eip155:4801"),
    ("xdai", "eip155:100"),
    ("xlayer-mainnet", "eip155:196"),
    ("xlayer-testnet", "eip155:195"),
    ("xrplevm-mainnet", "eip155:1440000"),
    ("xrplevm-testnet", "eip155:1449000"),
    ("zksync-mainnet", "eip155:324"),
    ("zksync-sepolia", "eip155:300"),
    ("zora-mainnet", "eip155:7777777"),
];

/// Resolves a `--pay-network` / config `pay_network` value to CAIP-2. Values
/// containing `:` pass through verbatim (Solana genesis-hash references are
/// case-sensitive, so no normalization is applied to them).
pub(super) fn resolve(input: &str) -> Result<String, CliError> {
    if input.contains(':') {
        return Ok(input.to_string());
    }
    let name = input.to_ascii_lowercase();
    match PAY_NETWORKS.binary_search_by_key(&name.as_str(), |(n, _)| n) {
        Ok(i) => Ok(PAY_NETWORKS[i].1.to_string()),
        Err(_) => Err(CliError::Arg(format!(
            "unknown pay network '{input}'. Use a Quicknode network name \
             (e.g. base-sepolia, solana-devnet, tempo-testnet) or a raw \
             CAIP-2 id (e.g. eip155:84532) — any eip155:<chain-id> or \
             solana:<genesis-hash> is accepted as-is"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        for pair in PAY_NETWORKS.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "PAY_NETWORKS out of order or duplicated at '{}'",
                pair[1].0
            );
        }
    }

    #[test]
    fn resolves_network_names() {
        assert_eq!(resolve("base-sepolia").unwrap(), "eip155:84532");
        assert_eq!(resolve("xdai").unwrap(), "eip155:100");
        assert_eq!(resolve("tempo-testnet").unwrap(), "eip155:42431");
        assert_eq!(
            resolve("solana-devnet").unwrap(),
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
        );
    }

    #[test]
    fn name_lookup_is_case_insensitive() {
        assert_eq!(resolve("Base-Sepolia").unwrap(), "eip155:84532");
    }

    #[test]
    fn caip2_passes_through_verbatim() {
        assert_eq!(resolve("eip155:84532").unwrap(), "eip155:84532");
        // Unknown-to-the-table but well-formed ids still work, unchanged.
        assert_eq!(
            resolve("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1").unwrap(),
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
        );
        assert_eq!(resolve("eip155:424242").unwrap(), "eip155:424242");
    }

    #[test]
    fn unknown_name_errors_with_escape_hatch() {
        let err = resolve("morph-hoodi").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("morph-hoodi"), "got: {msg}");
        assert!(msg.contains("eip155:"), "got: {msg}");
    }
}
