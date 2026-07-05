# P5b — Tokenized Treasury with Oracle-Priced NAV

The second half of the fifth mini-project in the
[RWA study plan](../../.claude/study-plan.md). It fuses **P5a** (fund shares +
subscribe/redeem + vault) with **P4** (safe Pyth price reading).

## The key mental shift vs P5a

P5a took deposits in **cash** ($1 = $1), so shares traded 1:1. Real funds take
deposits in assets whose **price moves**, so the shares you receive must come
from the deposit's **USD value**:

```
shares = USD_value_of_deposit / NAV_per_share
```

P5b makes the deposit a **volatile** asset (a mock wSOL) and reads its USD price
from **Pyth** — the exact staleness / confidence / feed-id checks from P4. NAV
per share is still $1; what's new is that the **deposit is now priced by an
oracle** instead of assumed to be a dollar.

| | P5a | P5b |
|---|---|---|
| Deposit asset | cash (mock USDC) | volatile (mock wSOL) |
| Exchange rate | fixed 1:1 | `oracle price × decimal scaling` |
| Decimals | must match | differ (9 vs 6), bridged by the math |
| New account per call | — | a Pyth `PriceUpdateV2` |

## The heart of P5b: the conversion math

A Pyth price is `mantissa × 10^exponent` USD per **whole** deposit token. Working
in raw base units at $1 NAV:

```
subscribe:  shares_raw  = deposit_amount × mantissa × 10^(exponent + share_dec − deposit_dec)
redeem:     deposit_raw = shares_raw × 10^(deposit_dec − share_dec − exponent) ÷ mantissa
```

`deposit_to_shares` and `shares_to_deposit` are **exact inverses**, which is why
a subscribe-then-redeem round trip at one price returns (almost) the original
deposit — the test's core assertion. The only loss is integer-division dust.

## What the program does (`programs/p5b-treasury-oracle/src/lib.rs`)

1. `initialize_fund` — same as P5a but **without** the equal-decimals rule; the
   oracle now bridges the two assets.
2. `subscribe(amount)` — validate the SOL/USD price → `deposit_to_shares` →
   user transfers wSOL to vault → PDA mints shares.
3. `redeem(shares)` — validate the price → `shares_to_deposit` → user burns
   shares → PDA pays wSOL out of the vault.
4. `load_validated_price` — P4's three checks, factored into one helper reused
   by both instructions.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — how P4 (oracle) and P5a (vault + subscribe/redeem) combine.
2. `load_validated_price` — this is P4 in miniature; make sure it's familiar.
3. `deposit_to_shares` / `shares_to_deposit` + `apply_pow10` — the exponent +
   decimals bookkeeping. Trace the shift for wSOL(9)/shares(6)/expo(−8).
4. `subscribe` then `redeem` — same two-CPI, who-signs-what pattern as P5a, now
   with the price computed first.
5. `tests/p5b-treasury-oracle.ts` — the price account is cloned from mainnet and
   the round trip returns ~the original 10 wSOL.

Good questions to bring: *Why compute shares from USD value instead of the raw
deposit amount? Walk the decimal shift: 10 wSOL at $150 → how many shares? Why
are the two conversions exact inverses, and where does the dust come from? What
happens to a redeemer if SOL's price MOVED between subscribe and redeem — who
gains or loses? Why must every subscribe/redeem carry a fresh price account?*

## Build & run

Toolchain (one-time, Homebrew — same as P1–P5a):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p5b-treasury-oracle && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p5b-treasury-oracle && anchor keys sync && anchor build && anchor test
```
Success = `3 passing`.

## ⚠️ Heads-up: two fragile spots (both expected)
1. **Crate versions** — P5b depends on **both** `anchor-spl` and
   `pyth-solana-receiver-sdk`; they must agree on the underlying
   `solana-program` version. If the first `cargo build` errors about duplicate /
   incompatible `solana-program` or `anchor` versions, paste me the error — a
   one-line pin, the code doesn't change.
2. **Cloned addresses** — the test clones the Pyth receiver program and a live
   SOL/USD price account from mainnet (see `Anchor.toml`). If `anchor test` says
   *account not found* or fails on staleness, that's the fix.

## Notes & honest simplifications
- The mock wSOL is just a token we pretend tracks SOL; the oracle prices "SOL"
  regardless. In production the deposit would be real wrapped SOL (or USDC + a
  USDC/USD feed).
- **Price risk is real here:** if SOL moves between subscribe and redeem, a
  redeemer gets more/less wSOL than they put in — because they hold *dollar*
  shares, not *SOL* shares. That's the correct behavior for a USD-NAV fund, and
  a good thing to reason through.
- NAV is still a fixed $1/share; a fuller model would also grow NAV over time
  (reconciled against the interest-bearing accrual from P5a).
- Program ID is a placeholder; `anchor keys sync` rewrites it.
