// ============================================================================
// P4 — Pyth Oracle Price Reader (safe on-chain price)
//
// THE BIG IDEA: a smart contract has no idea what SOL, BTC, or a treasury bill
// is worth in USD — the chain only knows token balances. An ORACLE bridges that
// gap: off-chain publishers agree on a price and post it on-chain, and our
// program reads it. Pyth is the price layer for Solana RWAs.
//
// Pyth on Solana is a "PULL" oracle (this is the mental model to hold):
//   - Prices are NOT continuously written to a fixed account by Pyth.
//   - Instead a CLIENT fetches a signed price update from Pyth's off-chain
//     service (Hermes), and POSTS it into a `PriceUpdateV2` account.
//   - OUR program is then handed that account and simply READS it.
//   So the freshness of the data is the *client's* responsibility to post, and
//   the *program's* responsibility to VALIDATE before trusting.
//
// Why "safe" price reading is the whole lesson: a raw price is dangerous. A
// price can be
//   1. STALE  — posted minutes ago; the market has moved. We reject old prices.
//   2. UNCERTAIN — Pyth ships every price with a confidence band (± conf). A
//      wide band means publishers disagree / low liquidity. We reject prices
//      whose band is too wide relative to the price.
//   3. WRONG FEED — the account might hold BTC/USD when we wanted SOL/USD. We
//      pin the exact feed id.
// Only a price that passes all three is trustworthy enough to price an asset.
//
// How P4 differs from P1–P3: those were all about TOKEN mechanics (mint, freeze,
// transfer rules). P4 touches no tokens at all — it is purely about getting a
// trustworthy real-world number on-chain. P5 will combine the two: price a
// yield-bearing token at its NAV using exactly this pattern.
// ============================================================================

use anchor_lang::prelude::*;

// The Pyth pull-oracle SDK. Two things we use from it:
//   - `PriceUpdateV2`: the Anchor account type the client posts the price into.
//     Declaring it as `Account<'info, PriceUpdateV2>` makes Anchor verify the
//     account is really owned by the Pyth receiver program (ownership check for
//     free — you can't hand us a fake account from some other program).
//   - `get_feed_id_from_hex`: turns the 32-byte hex feed id (from the Pyth
//     price-feeds page) into the [u8; 32] the SDK compares against.
use pyth_solana_receiver_sdk::price_update::{get_feed_id_from_hex, PriceUpdateV2};

declare_id!("OracleP4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

// --- Our safety policy (the knobs a real integration tunes per asset) ---------

// Reject any price whose publish time is older than this many seconds. 60s is
// generous for a demo; a liquidation engine might use 10–25s.
const MAX_PRICE_AGE_SECS: u64 = 60;

// Reject any price whose confidence band is wider than this fraction of the
// price, expressed in basis points (1 bp = 0.01%). 100 bps = 1%. If Pyth says
// "SOL is $150 ± $3", that band is 2% = 200 bps and we'd reject it as too noisy.
const MAX_CONFIDENCE_BPS: u128 = 100;

// The Pyth feed id for SOL/USD (hex, from https://pyth.network/developers/price-feed-ids).
// This pins WHICH asset's price we accept — a BTC/USD update in the account
// would fail the feed-id check inside `get_price_no_older_than`.
const SOL_USD_FEED_ID: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

#[program]
pub mod p4_pyth_oracle {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction: read_price
    //
    // Reads the posted SOL/USD price update, applies all three safety checks,
    // and logs the price. Returns an error (reverting the tx) if the price is
    // stale, too uncertain, or from the wrong feed.
    // ------------------------------------------------------------------------
    pub fn read_price(ctx: Context<ReadPrice>) -> Result<()> {
        let price_update = &ctx.accounts.price_update;

        // CHECK 1 (staleness) + CHECK 3 (wrong feed), both handled by the SDK:
        // `get_price_no_older_than` fails if the update is older than
        // MAX_PRICE_AGE_SECS *or* if the account holds a different feed than the
        // id we pass. Clock::get() is the on-chain wall clock it compares against.
        let feed_id = get_feed_id_from_hex(SOL_USD_FEED_ID)?;
        let price = price_update.get_price_no_older_than(
            &Clock::get()?,
            MAX_PRICE_AGE_SECS,
            &feed_id,
        )?;

        // A Pyth price is an integer `price` plus a base-10 `exponent`:
        //   real_value = price * 10^exponent      (exponent is usually -8)
        // e.g. price = 15000000000, exponent = -8  ->  $150.00000000
        // `conf` is the ± confidence band in the SAME integer units as `price`.

        // CHECK 2 (confidence): reject if the ± band is too wide relative to the
        // price. We compare in basis points so it's exponent-independent (conf
        // and price share the same exponent, so the ratio cancels it out).
        require!(price.price > 0, OracleError::NonPositivePrice);
        let conf_bps = (price.conf as u128)
            .checked_mul(10_000)
            .and_then(|v| v.checked_div(price.price as u128))
            .ok_or(OracleError::MathOverflow)?;
        require!(conf_bps <= MAX_CONFIDENCE_BPS, OracleError::PriceTooUncertain);

        // Normalize to a fixed 6-decimal USD integer for a human-readable log
        // (this is the exponent handling you'll reuse everywhere in P5/P6).
        let usd_6dp = to_fixed_point(price.price, price.exponent, 6)?;

        msg!(
            "SOL/USD = {} (raw {} x 10^{}), band +/-{} ({} bps), age <= {}s",
            format_6dp(usd_6dp),
            price.price,
            price.exponent,
            price.conf,
            conf_bps,
            MAX_PRICE_AGE_SECS
        );

        Ok(())
    }
}

// Convert a Pyth (mantissa, exponent) price into an integer with exactly
// `target_decimals` decimal places. Pyth exponents are negative (e.g. -8), so
// we scale the mantissa by 10^(exponent + target_decimals): shift right if that
// is still negative, left if positive. This is the one bit of arithmetic every
// oracle consumer needs, so it lives in its own function.
fn to_fixed_point(mantissa: i64, exponent: i32, target_decimals: i32) -> Result<u64> {
    let shift = exponent + target_decimals; // e.g. -8 + 6 = -2
    let m = mantissa as i128;
    let scaled: i128 = if shift >= 0 {
        m.checked_mul(10i128.pow(shift as u32))
            .ok_or(OracleError::MathOverflow)?
    } else {
        m.checked_div(10i128.pow((-shift) as u32))
            .ok_or(OracleError::MathOverflow)?
    };
    u64::try_from(scaled).map_err(|_| OracleError::MathOverflow.into())
}

// Pretty-print a 6-decimal integer as "123.456789" for the log message.
fn format_6dp(value: u64) -> String {
    format!("{}.{:06}", value / 1_000_000, value % 1_000_000)
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct ReadPrice<'info> {
    // The price update account the client posted. Typing it as
    // `Account<'info, PriceUpdateV2>` makes Anchor enforce that it is genuinely
    // owned by the Pyth receiver program — the deserialization + ownership check
    // is exactly why we don't have to trust arbitrary account data by hand.
    pub price_update: Account<'info, PriceUpdateV2>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum OracleError {
    #[msg("Price confidence band is too wide to trust")]
    PriceTooUncertain,
    #[msg("Price is zero or negative")]
    NonPositivePrice,
    #[msg("Arithmetic overflow while scaling the price")]
    MathOverflow,
}
