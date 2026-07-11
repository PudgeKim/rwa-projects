# P7 — Capstone: A Full Compliant RWA Token

The final mini-project in the [RWA study plan](../../.claude/study-plan.md). It
puts the **whole plan on one token**: a single Token-2022 mint that is
KYC-gated, yield-bearing, oracle-priced, clawback-able — and (via the referenced
hook + AMM) compliance-checked on every transfer and freely tradeable.

## Everything, on one mint

| Feature | From | Mechanism |
|---|---|---|
| New holders **frozen until KYC** | P2 | Default Account State = Frozen + `verify_kyc` (thaw) |
| Balance **accrues yield** | P5a | Interest-Bearing extension |
| Issuer can **claw back** | P3b | Permanent Delegate |
| Subscribe/redeem **priced safely** | P4 + P5b | Pyth SOL/USD, staleness/confidence checks |
| Allowlist **on every transfer** | P3a | *separate* transfer-hook program (referenced) |
| **Traded** on a market | P6 | *separate* constant-product AMM (referenced) |

### The one PDA to rule them all

`seeds = ["authority"]` is at once the **mint authority** (issue shares), the
**freeze authority** (thaw = KYC), the **permanent delegate** (claw back), and
the **vault owner** (custody deposits). Every privileged action is a CPI the
program signs with that single PDA — the thread that ties the capstone together.

## The compliance story (what the test walks through)

```
new holder's share account  ──created FROZEN──►  cannot receive shares
        │  admin: verify_kyc (thaw)
        ▼
   KYC-approved  ──subscribe──►  oracle-priced, yield-bearing shares
        │  (holds; balance accrues via interest-bearing)
        │  issuer: clawback (permanent delegate, no holder signature)
        ▼
   shares force-moved to a recovery account
```

The neat part: `subscribe` needs **no explicit KYC check** — minting into a
frozen account simply fails, so the freeze *is* the gate. Compliance for free.

## What the program does (`programs/p7-compliant-rwa/src/lib.rs`)

1. `initialize_fund` — record mints, create the deposit vault.
2. `verify_kyc` — thaw a holder's share account (P2).
3. `subscribe(amount)` — oracle-priced issuance; fails if not KYC'd (P4/P5b).
4. `redeem(shares)` — burn shares, pay out the priced deposit (P5b).
5. `clawback(amount)` — permanent-delegate force-transfer, admin-gated (P3b).

## Reading guide

1. Top comment — the extension table and the one-PDA idea.
2. `verify_kyc` — the thaw CPI; this is the compliance switch.
3. `subscribe` — note there's no KYC `require!`; the frozen mint enforces it.
4. `clawback` — the permanent-delegate transfer with no holder signature.
5. The pricing helpers — identical to P5b.
6. `tests/…ts` — the frozen → fail → KYC → succeed → clawback sequence.

## How the two decoupled pieces plug in

- **Transfer-hook allowlist (P3a):** this mint could *also* set a Transfer Hook
  pointing at the P3a program, so **every transfer** (not just issuance) checks
  an allowlist. It's a separate program because the hook is decoupled from the
  mint — the mint just names it, and Token-2022 CPIs into it on each transfer.
  Freeze (P2) gates *account activation*; the hook gates *each transfer*. Real
  RWAs use both.
- **AMM trading (P6):** the compliant token is one side of a P6b pool (vs USDC).
  Its **market price** (pool ratio) is kept in line with its **official price**
  (P6a oracle / this fund's NAV) by arbitrage. Note: if the token has an active
  transfer hook/allowlist, the pool's vaults and traders must themselves be
  allowlisted — which is exactly why permissioned RWAs trade in *permissioned*
  pools.

## How a venue would custody & list this (the plan's closing reading)

- **Kamino (DeFi lending/vaults):** would onboard the token as collateral only
  after modeling its oracle (staleness/confidence, like P4), its **redemption
  liquidity** (can the vault honor redeems?), and its **clawback risk** — a
  permanent delegate means positions can be seized, which a lending market must
  price in or gate to allowlisted users.
- **Backpack / Kraken (CEX):** custody the token in exchange-controlled wallets
  that are **KYC'd/allowlisted** (they pass the freeze + hook checks), map it to
  a fiat/stablecoin order book, and handle corporate actions (yield accrual,
  clawbacks) operationally. The exchange's own compliance replaces on-chain
  gating for users trading *inside* the venue.
- **Common thread:** every venue cares about the same four things this token
  encodes — *who may hold it* (KYC/hook), *what it's worth* (oracle/NAV), *how
  yield accrues* (interest-bearing), and *how the issuer can intervene*
  (clawback). The capstone is exactly those four made concrete.

## Build & run

```bash
! cd /Users/kanetla/Documents/rwa/projects/p7-compliant-rwa && npm install
! cd /Users/kanetla/Documents/rwa/projects/p7-compliant-rwa && anchor keys sync && anchor build && anchor test
```
Success = `4 passing`.

## ⚠️ Heads-up (same as P4/P5b/P6a)
- **Crate versions:** `anchor-spl` + `pyth-solana-receiver-sdk` must agree on the
  underlying `solana-program`. Paste me any first-build version error.
- **Cloned addresses:** the test clones the Pyth receiver + a SOL/USD account
  from mainnet (`Anchor.toml`). If it says *account not found*, that's the fix.

## Notes & honest simplifications
- **Feed stand-in:** SOL/USD (24/7) prices the deposit; a real fund would price
  the *underlying* and grow NAV over time (reconciled with the interest accrual).
- **Clawback needs a thawed source & destination:** a frozen account blocks even
  the permanent delegate, so the test thaws the recovery account. A production
  flow would thaw → claw → (optionally) re-freeze.
- The transfer-hook allowlist (P3a) is **referenced, not wired in**, to keep the
  capstone to one readable program — adding it is the natural "graduation" step.
- Program ID is a placeholder; `anchor keys sync` rewrites it.
