# P5a — Yield-bearing Tokenized Treasury (fixed $1 NAV)

The first half of the fifth mini-project in the
[RWA study plan](../../.claude/study-plan.md). A **tokenized treasury** (Ondo
USDY, Superstate USTB, BlackRock BUIDL) is a token that represents a **share of
a fund** holding real T-bills. Two things define it:

1. It **accrues yield** — a share is worth a little more each day.
2. You **subscribe** (cash → shares) and **redeem** (shares → cash) at the
   fund's **NAV** (net asset value per share).

P5a builds the subscribe/redeem machinery at a **fixed $1 NAV** and represents
the yield with Token-2022's **interest-bearing** extension. **P5b** will replace
the fixed price with a live **Pyth oracle** (reusing P4).

## The key mental shift vs P1–P4

P2/P3 were compliance on a *single* token. P5a is the first project that **moves
value between two tokens** and **custodies funds**:

| | P2/P3/P3b | P5a |
|---|---|---|
| Tokens involved | one | **two** — cash (mock USDC) + fund shares |
| Core action | gate / recover a token | **swap** cash ↔ shares at NAV |
| New machinery | — | a **vault** (program-owned account) + a PDA that owns the vault *and* mints shares |

## Where the yield actually comes from (the important part)

The share mint is created **with the interest-bearing extension** and a rate
(5% in the test). That extension **does not mint new tokens**. It only changes
the **displayed** balance:

```
uiAmount = rawAmount grown continuously by the rate since it was minted
```

So a holder's **raw** amount is constant, but their **UI** (spendable-value)
amount creeps up — *that* is the yield. Our program only ever touches **raw**
amounts (mint / burn / transfer), which is exactly how a real fund accounts for
shares. The growth is a property of the mint, not something the program does per
transaction.

## What the program does (`programs/p5a-treasury-fund/src/lib.rs`)

1. `initialize_fund` — record the two mints and create the **vault** (a
   program-owned cash account, owned by the `authority` PDA). Requires both
   mints to share decimals so the 1:1 (=$1 NAV) math is exact.
2. `subscribe(amount)` — two CPIs: **user signs** a transfer of cash → vault,
   then the **PDA signs** a mint of shares → user.
3. `redeem(shares)` — the reverse: **user signs** a burn of shares, then the
   **PDA signs** a transfer of cash vault → user.

One PDA (`["authority"]`) is both the **vault owner** and the **share mint
authority** — so a single program signer authorizes everything the program does.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — the subscribe/redeem flow and the raw-vs-UI-amount idea.
2. `subscribe` — note the two CPIs and **who signs each**: the user signs the
   cash transfer (their money), the PDA signs the mint (only the program may mint).
3. `redeem` — the mirror image; the PDA now signs the payout from the vault.
4. `InitializeFund` accounts — how the `vault` is created and why one PDA owns
   both the vault and the mint.
5. `Subscribe`/`Redeem` accounts — the `has_one` checks that pin the mints and
   vault to the config so nothing can be swapped out.
6. `tests/p5a-treasury-fund.ts` — how the share mint gets the interest-bearing
   extension, and the full 1000-in / 400-out cycle.

Good questions to bring: *Why does subscribe need TWO signers' worth of
authority (user for the transfer, PDA for the mint)? Why is the vault owned by a
PDA instead of the admin? What's the difference between a share's raw amount and
its UI amount — and which one does redemption use? If NAV is fixed at $1, where
is the yield actually paid when I redeem? (Honest answer below.) Why require the
two mints to have equal decimals?*

## Build & run

Toolchain (one-time, Homebrew — same as P1–P4):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p5a-treasury-fund && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p5a-treasury-fund && anchor keys sync && anchor build && anchor test
```
Success = `5 passing`.

## Notes & honest simplifications
- **Where's the yield at redemption?** With a fixed $1 NAV and 1:1 raw
  redemption, this demo pays back exactly the principal — the interest-bearing
  extension grows the *displayed* value but P5a doesn't reconcile that at
  redeem. A real fund redeems against the **accrued** value (UI amount) and
  holds enough reserves to cover it. We keep it 1:1 so the mechanics stay
  readable; P5b moves NAV off a fixed constant.
- Interest accrues over real time, so in a seconds-long test the UI amount
  barely moves — the test asserts the **rate is configured**, not visible growth.
- The admin and the subscribing user are the same wallet here purely to keep the
  test short; nothing in the program requires that.
- Program ID is a placeholder; `anchor keys sync` rewrites it.
