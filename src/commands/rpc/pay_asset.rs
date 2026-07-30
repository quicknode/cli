//! Human-readable symbols for `--payment-asset`.
//!
//! The SDK's `PaymentConfig.asset` is a token contract address (EVM) or mint
//! (Solana) — the exact value an x402/MPP offer is matched against. A symbol
//! like `USDC` only identifies a concrete address once the network is known:
//! USDC is a different address on every chain. So this resolver takes the
//! already-resolved CAIP-2 network alongside the input and maps a known symbol
//! to that network's address.
//!
//! Unlike `pay_network`, this is intentionally permissive: an asset is an
//! open-ended address across many chains, not a closed Quicknode vocabulary.
//! Anything that isn't a recognized symbol passes through verbatim, so any
//! token address reaches the gateway unchanged and the gateway stays the
//! authority on whether it is a valid, payable asset. Only symbols confirmed
//! against a public source (the gateway's own discovery catalog) are listed —
//! a wrong address could pay in the wrong token.

use crate::errors::CliError;

/// (CAIP-2 network, lowercase symbol) → token address. Sorted by the
/// `(network, symbol)` tuple (binary searched); a unit test enforces order and
/// uniqueness. Addresses are taken from a public source — the payment
/// gateways' discovery catalog (`qn rpc x402 supported-networks`) or, for
/// Tempo, the MPP spec / Tempo payment docs.
const PAY_ASSETS: &[(&str, &str, &str)] = &[
    (
        "eip155:196",
        "usdc",
        "0x4ae46a509F6b1D9056937BA4500cb143933D2dc8",
    ),
    // Tempo pathUSD and USDC.e (bridged USDC, mapped from the `usdc` symbol),
    // from the MPP spec / Tempo payment docs and the MPP gateway's own 402
    // challenge. Testnet `usdc` resolves to pathUSD (the testnet stand-in).
    (
        "eip155:4217",
        "pathusd",
        "0x20c0000000000000000000000000000000000000",
    ),
    (
        "eip155:4217",
        "usdc",
        "0x20c000000000000000000000b9537d11c60e8b50",
    ),
    (
        "eip155:42431",
        "pathusd",
        "0x20c0000000000000000000000000000000000000",
    ),
    (
        "eip155:42431",
        "usdc",
        "0x20c0000000000000000000000000000000000000",
    ),
    (
        "eip155:8453",
        "usdc",
        "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    ),
    (
        "eip155:84532",
        "usdc",
        "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    ),
    (
        "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
        "usdc",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    ),
    (
        "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        "usdc",
        "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU",
    ),
];

/// The symbols this resolver recognizes, lowercased. Input matching one of
/// these (case-insensitively) is rewritten to an address for the given
/// network, or errors if that network has no mapping. Anything else passes
/// through verbatim as an address.
const KNOWN_SYMBOLS: &[&str] = &["pathusd", "usdc"];

/// Resolves a `--payment-asset` / config `payment_asset` value to a token
/// address, given the already-resolved CAIP-2 `network`. A recognized symbol
/// (e.g. `USDC`, case-insensitive) is mapped to that network's address; every
/// other value passes through unchanged.
pub(super) fn resolve(input: &str, network: &str) -> Result<String, CliError> {
    let lower = input.to_ascii_lowercase();
    if !KNOWN_SYMBOLS.contains(&lower.as_str()) {
        return Ok(input.to_string());
    }
    match PAY_ASSETS.binary_search_by(|(net, sym, _)| (*net, *sym).cmp(&(network, lower.as_str())))
    {
        Ok(i) => Ok(PAY_ASSETS[i].2.to_string()),
        Err(_) => Err(CliError::Arg(format!(
            "no known {} address for network '{network}'. Pass the token \
             contract address (EVM) or mint (Solana) directly to \
             --payment-asset — run 'qn rpc x402 supported-networks' or \
             'qn rpc mpp supported-networks' to find it",
            input.to_ascii_uppercase()
        ))),
    }
}

/// Reverse lookup: a resolved token address on `network` → its known symbol,
/// in display casing. Used to name the asset in funds-moving prompts and the
/// supported-networks tables instead of echoing the raw contract address. EVM
/// hex compares case-insensitively (checksum casing varies); a miss just means
/// the caller shows the address.
pub(super) fn symbol_for(network: &str, address: &str) -> Option<String> {
    PAY_ASSETS
        .iter()
        .find(|(net, _, addr)| *net == network && addr.eq_ignore_ascii_case(address))
        .map(|(_, sym, _)| display_symbol(sym))
}

/// Display casing for a lowercase table symbol (`pathusd` → `pathUSD`).
fn display_symbol(sym: &str) -> String {
    match sym {
        "pathusd" => "pathUSD".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        for pair in PAY_ASSETS.windows(2) {
            let a = (pair[0].0, pair[0].1);
            let b = (pair[1].0, pair[1].1);
            assert!(a < b, "PAY_ASSETS out of order or duplicated at {b:?}");
        }
    }

    #[test]
    fn resolves_symbol_per_network() {
        assert_eq!(
            resolve("usdc", "eip155:84532").unwrap(),
            "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
        );
        assert_eq!(
            resolve("usdc", "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").unwrap(),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        // Tempo (MPP): mainnet USDC and testnet pathUSD.
        assert_eq!(
            resolve("usdc", "eip155:4217").unwrap(),
            "0x20c000000000000000000000b9537d11c60e8b50"
        );
        assert_eq!(
            resolve("usdc", "eip155:42431").unwrap(),
            "0x20c0000000000000000000000000000000000000"
        );
    }

    #[test]
    fn symbol_lookup_is_case_insensitive() {
        assert_eq!(
            resolve("USDC", "eip155:8453").unwrap(),
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        );
    }

    #[test]
    fn address_passes_through_verbatim() {
        let addr = "0x036CbD53842c5426634e7929541eC2318f3dCF7e";
        assert_eq!(resolve(addr, "eip155:84532").unwrap(), addr);
        // A base58 mint (unknown to the table) is untouched.
        let mint = "So11111111111111111111111111111111111111112";
        assert_eq!(
            resolve(mint, "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").unwrap(),
            mint
        );
    }

    #[test]
    fn symbol_for_reverses_known_addresses() {
        assert_eq!(
            symbol_for("eip155:84532", "0x036CbD53842c5426634e7929541eC2318f3dCF7e").as_deref(),
            Some("USDC")
        );
        // Checksum casing differences still match.
        assert_eq!(
            symbol_for("eip155:84532", "0x036cbd53842c5426634e7929541ec2318f3dcf7e").as_deref(),
            Some("USDC")
        );
        // Unknown address, or a known address on the wrong network, is a miss.
        assert_eq!(symbol_for("eip155:84532", "0xabc"), None);
        assert_eq!(
            symbol_for("eip155:1", "0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
            None
        );
    }

    #[test]
    fn known_symbol_on_unmapped_network_errors() {
        let err = resolve("usdc", "eip155:1").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("USDC"), "got: {msg}");
        assert!(msg.contains("eip155:1"), "got: {msg}");
    }
}
