# P4 — Pyth Oracle Price Reader (safe price)

The fourth mini-project in the [RWA study plan](../../.claude/study-plan.md).
Goal: bring a **real-world price on-chain safely**. A smart contract only knows
token balances — it has no idea what SOL or a treasury bill is worth. An
**oracle** fills that gap: publishers agree on a price and post it on-chain, and
our program reads it. This is the **price layer** every later project needs.

## The key mental shift vs P1–P3

P1–P3 were all about **token mechanics** (mint, freeze, transfer rules). P4
touches **no tokens at all**. It is purely about getting a *trustworthy number*
on-chain — and the whole lesson is that a raw price is **not** trustworthy until
you validate it.

| | P2/P3 (token rules) | P4 (oracle) |
|---|---|---|
| What we manipulate | token accounts | a price number |
| Where the data lives | accounts our program owns | a `PriceUpdateV2` account **someone else posted** |
| Our program's job | enforce a rule on tokens | **validate then read** a price |

## Pull oracle — the model to hold in your head

Pyth on Solana is a **pull** oracle:

1. Pyth does **not** continuously write prices to a fixed account.
2. A **client** fetches a signed price update from Pyth's off-chain service
   (Hermes) and **posts** it into a `PriceUpdateV2` account.
3. **Our program is handed that account and only READS it.**

So freshness is the client's job to *post* and our job to *validate*. In this
project the test's `Anchor.toml` **clones a live SOL/USD account from mainnet**
to play the role of "someone already posted a fresh price."

## What the program does (`programs/p4-pyth-oracle/src/lib.rs`)

One instruction, `read_price`, applies **three safety checks** — the reason this
project exists:

1. **Staleness** — reject a price older than `MAX_PRICE_AGE_SECS`. The market
   moves; an old price is a wrong price.
2. **Wrong feed** — pin the exact SOL/USD **feed id**, so a BTC/USD update
   sitting in the account is rejected. (Checks 1 and 3 are both done by the
   SDK's `get_price_no_older_than`.)
3. **Confidence** — Pyth ships every price as `price ± conf`. A wide band means
   publishers disagree / thin liquidity. We reject if the band is wider than
   `MAX_CONFIDENCE_BPS` of the price.

It then normalizes the price using its **exponent** (`real = price * 10^expo`)
and logs it. That exponent handling (`to_fixed_point`) is the one arithmetic
pattern you'll reuse in P5/P6.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — the **pull-oracle** flow (who posts, who reads).
2. The three `const`s — our safety policy (age, confidence, feed id).
3. `read_price` — the three checks in order; note that `get_price_no_older_than`
   quietly does staleness **and** feed-id matching for you.
4. `to_fixed_point` — the exponent math (Pyth's classic gotcha).
5. `ReadPrice` accounts — why `Account<'info, PriceUpdateV2>` gives you an
   ownership check for free.
6. `tests/p4-pyth-oracle.ts` — how the price account address is **derived** and
   why `Anchor.toml` clones it from mainnet.

Good questions to bring: *Why is a raw price dangerous? What is the confidence
band and why reject a wide one? Why does staleness need the on-chain clock?
What does the exponent mean and why is it negative? Why is the price account
posted by a client instead of written by Pyth directly? What stops me passing an
account from a different program as the price update?*

## Build & run

Toolchain (one-time, Homebrew — same as P1–P3):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p4-pyth-oracle && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p4-pyth-oracle && anchor keys sync && anchor build && anchor test
```

## ⚠️ Heads-up: two fragile spots (both expected, both quick to fix)

1. **Crate version** — `pyth-solana-receiver-sdk` must agree with the
   `solana-program` that `anchor-lang 1.0.2` pulls in (same version-sensitivity
   as P3's transfer-hook crates). If the **first `cargo build`** errors about
   duplicate/incompatible `solana-program` or `anchor` versions, **paste me the
   error** — it's a one-line pin, the code doesn't change.
2. **Cloned addresses** — the test clones the Pyth receiver program and a live
   SOL/USD price account from mainnet (see `Anchor.toml`). If `anchor test` says
   *account not found* or the read fails on staleness, the address/cluster is the
   fix — tell me and we'll update them from the Pyth docs.

## Notes
- Program ID is a placeholder; `anchor keys sync` rewrites it.
- To **see the staleness check bite**, drop `MAX_PRICE_AGE_SECS` to `1` and
  re-run: the cloned account will now read as too old and the tx reverts.
- P4 reads a price but does nothing with it. **P5** wires this exact pattern into
  a token: mint/redeem a yield-bearing treasury token at its oracle-priced NAV.
