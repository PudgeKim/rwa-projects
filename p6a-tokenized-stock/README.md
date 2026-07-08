# P6a — Tokenized Stock Issuance (primary market)

The first half of the sixth mini-project in the
[RWA study plan](../../.claude/study-plan.md). A **tokenized stock** (xStocks'
AAPLx, Backed's bCSPX) is a token where **1 token = 1 share** of a real stock in
custody. P6a builds the **primary market**: the issuer lets you **buy** shares
by paying oracle-priced USDC, and **sell** them back.

**P6b** adds the **secondary market** — a mini AMM where the token trades freely
and its price is set by supply/demand instead of the oracle.

## The key mental shift vs P5b

The oracle + vault + mint/burn machinery is the same family as the P5b treasury,
but the **direction flips**:

| | P5b (treasury fund) | P6a (tokenized stock) |
|---|---|---|
| You specify | a deposit amount | how many **shares** you want |
| Priced thing | the volatile *deposit* | the *share* itself |
| Formula | shares = deposit·price / $1 | **cost = shares · price** |
| Pay with | volatile asset | USDC (stable) |

So P6a reinforces P5b's pricing but from the buyer's side: *name the shares, pay
the oracle-priced cost.*

## What the program does (`programs/p6a-tokenized-stock/src/lib.rs`)

1. `initialize_market` — record the stock + USDC mints, create the USDC vault
   (owned by the `authority` PDA, which is also the stock mint authority).
2. `buy(shares)` — validate price → `cost = shares·price` → user pays USDC to
   vault → PDA mints shares to user.
3. `sell(shares)` — validate price → `proceeds = shares·price` → user burns
   shares → PDA pays USDC out of the vault.

`buy` and `sell` share one `Trade` accounts struct because they touch the same
accounts — only the direction of the two transfers differs.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — primary vs secondary market, and the P5b direction-flip.
2. `shares_to_usdc` — the one conversion (shares → USDC), same decimals+exponent
   bookkeeping as P5b.
3. `buy` then `sell` — the familiar two-CPI, who-signs-what pattern.
4. `tests/…ts` — buy 2 shares then sell them back for the exact same USDC.

Good questions to bring: *Why is the round trip EXACT here (no dust) when P5b's
had rounding? Who backs the shares — what stops the issuer minting infinite
tokens? What happens if the oracle price moves between buy and sell? How will
this "official" price relate to the AMM price in P6b?*

## Build & run

```bash
! cd /Users/kanetla/Documents/rwa/projects/p6a-tokenized-stock && npm install
! cd /Users/kanetla/Documents/rwa/projects/p6a-tokenized-stock && anchor keys sync && anchor build && anchor test
```
Success = `3 passing`.

## ⚠️ Heads-up (same as P4/P5b)
- **Crate versions:** `anchor-spl` + `pyth-solana-receiver-sdk` must agree on the
  underlying `solana-program`. Paste me any first-build version error.
- **Cloned addresses:** the test clones the Pyth receiver + a SOL/USD account
  from mainnet (`Anchor.toml`). If it says *account not found*, that's the fix.

## Notes & honest simplifications
- **Feed stand-in:** real tokenized stocks use an equity feed (AAPL/USD), which
  only updates during market hours — flaky for a localnet test. We reuse the
  24/7 **SOL/USD** feed as the "share price"; only the feed id changes in prod.
- No fee/spread here (kept clean); P6b introduces a swap fee.
- The vault must hold enough USDC to honor `sell` — in this demo it's funded by
  prior `buy`s; a real issuer holds reserves.
- Program ID is a placeholder; `anchor keys sync` rewrites it.
