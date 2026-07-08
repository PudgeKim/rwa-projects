# P6b — Mini Constant-Product AMM (secondary market)

The second half of the sixth mini-project in the
[RWA study plan](../../.claude/study-plan.md). P6a was the **primary market**
(the issuer mints/redeems the stock at the oracle price). P6b is the **secondary
market** — an Automated Market Maker where the token trades freely and its price
comes from a **pool of reserves**, with no oracle and no order book. This is how
xStocks tokens actually trade on Orca / Raydium / Jupiter.

## The one invariant everything rests on

```
reserve_a * reserve_b = k     (a swap must not let k decrease)
```

- **Price** is just the ratio `reserve_b / reserve_a`. Buying A removes A and
  adds B, so A gets scarcer and pricier — the pool self-adjusts, no oracle needed.
- **Liquidity providers** deposit both tokens in the current ratio and get **LP
  tokens** for their share; they earn the swap fee.
- **A swap** puts `amount_in` of one side in and takes `amount_out` of the other
  out, keeping the product ~constant. A fee (0.3% here) is skimmed from the
  input — that's what pays the LPs.

## How this ties the whole P6 together

P6a gives an **official** price (the oracle); P6b gives a **market** price (the
pool ratio). If they diverge, **arbitrageurs** buy on the cheap side and sell on
the dear side until they meet. That arbitrage is what keeps a tokenized stock's
traded price pinned to the real share price — the key insight of P6.

## What the program does (`programs/p6b-mini-amm/src/lib.rs`)

1. `initialize_pool(fee_bps)` — create the pool + two reserve vaults (owned by
   the `authority` PDA, which is also the LP mint authority).
2. `add_liquidity(amount_a, max_b, min_lp)` — first deposit sets the price and
   mints `sqrt(a*b)` LP; later deposits must match the reserve ratio.
3. `remove_liquidity(lp, min_a, min_b)` — burn LP, get a proportional slice of
   both reserves.
4. `swap(amount_in, min_out, a_to_b)` — the `x*y=k` trade with a fee and a
   `min_out` slippage guard.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — the constant-product invariant and the arbitrage link to P6a.
2. `get_amount_out` — the core swap formula; see exactly where the fee enters.
3. `swap` — how direction (`a_to_b`) picks the in/out reserves, and who signs
   each transfer (user in, PDA out).
4. `add_liquidity` — the first-vs-later branch and why LP = `sqrt(a*b)` first.
5. `remove_liquidity` — the proportional payout.
6. `tests/…ts` — a full lifecycle and the assertion that `k` never shrinks.

Good questions to bring: *Why does the price move as you trade — where does
slippage come from? Why `sqrt(a*b)` for the first LP mint? Why does the constant
product actually GROW a little on each swap (hint: the fee)? What is
impermanent loss, and where would an LP feel it here? Why is `min_out` essential
(what attack does it stop)?*

## Build & run

```bash
! cd /Users/kanetla/Documents/rwa/projects/p6b-mini-amm && npm install
! cd /Users/kanetla/Documents/rwa/projects/p6b-mini-amm && anchor keys sync && anchor build && anchor test
```
Success = `4 passing`. No oracle here, so there's nothing to clone — it runs on
a plain local validator.

## Notes & honest simplifications
- **No `MINIMUM_LIQUIDITY` lock.** Real Uniswap v2 burns the first ~1000 LP to
  prevent a share-price manipulation on an empty pool. We skip it for
  readability; it's a good follow-up to add.
- **Later `add_liquidity` anchors on token A** and derives the required B; a
  fuller version lets you anchor on either side and refunds the remainder.
- Both tokens and the LP token are Token-2022 mints here so one token program
  handles everything.
- Program ID is a placeholder; `anchor keys sync` rewrites it.
