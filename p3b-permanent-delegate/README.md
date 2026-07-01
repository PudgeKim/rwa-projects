# P3b — Permanent Delegate Clawback (issuer recovers tokens)

The second half of the third mini-project in the
[RWA study plan](../../.claude/study-plan.md). P3a asked *who may **receive**?*
(the transfer hook). P3b asks the other compliance question:

> *How does the issuer **take tokens back** from a holder?*

A regulated real-world asset sometimes **must** be recovered — a court order,
sanctions, a compromised wallet, a failed KYC recheck. A normal SPL token can't
do this: only the owner can move their tokens. Token-2022's **Permanent
Delegate** extension is the answer — a mint names one pubkey that may
`transfer` or `burn` from **any** account of that mint, **without** the holder's
signature.

## The key mental shift vs P3a

| | P3a (transfer hook) | P3b (permanent delegate) |
|---|---|---|
| Question | who may **receive**? | how does the issuer **take back**? |
| Mechanism | a separate rule-checker program | a mint-level **extension** |
| The power | reject a transfer | move/burn from **any** account, no owner signature |
| Where it's configured | mint points at hook program | permanent delegate set at mint creation |

Together, P3a + P3b are the two sides of a compliant token: **gate who can hold
it**, and **let the issuer recover it**.

## The design: why a program, not just an admin keypair

The permanent delegate could be a plain wallet — but then that key alone is the
clawback power, with no gate. Instead we make the delegate a **PDA of this
program** (seeds `["delegate"]`). Because it's program-derived, **only this
program can sign as it** (via `with_signer(seeds)` in a CPI). So clawback can
only happen through our `clawback` instruction, which we gate with an admin
check (`has_one = admin`, same as P2's Config). A raw dangerous power becomes a
controlled, auditable one.

## What the program does (`programs/p3b-permanent-delegate/src/lib.rs`)

1. `initialize_config` — store the `admin` allowed to trigger recovery (a plain
   data account, like P2's Config).
2. `clawback` — admin force-moves tokens from any `from` account to a recovery
   `to` account, via a `transfer_checked` CPI **signed by the delegate PDA**.
3. `burn_from` — the other delegate power: destroy tokens outright (sanctions),
   via a `burn` CPI signed by the same PDA.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — the runtime flow (admin → our gate → CPI signed by the delegate
   PDA → Token-2022 allows it because the PDA *is* the permanent delegate).
2. `clawback` — the `with_signer(seeds)` CPI. This is the crux: the PDA signature
   is what lets us move someone else's tokens.
3. `Clawback` accounts — notice there is **no `Signer` for the victim**; only
   `admin` signs.
4. `burn_from` — same pattern, `burn` instead of `transfer_checked`.
5. `tests/p3b-permanent-delegate.ts` — the mint is created with
   `createInitializePermanentDelegateInstruction` pointing at our PDA; then the
   admin claws back and burns while the victim's keypair signs **nothing**.

Good questions to bring: *Why make the delegate a PDA instead of the admin's own
wallet? What exactly authorizes moving another account's tokens — where's the
"permission" checked? Could the victim block or detect this? What's the
difference between clawback and burn, and when would an issuer use each? Can the
permanent delegate be changed or removed after mint creation?*

## Build & run

Toolchain (one-time, Homebrew — same as P1–P3a):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p3b-permanent-delegate && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p3b-permanent-delegate && anchor keys sync && anchor build && anchor test
```
Success = `4 passing`.

## Notes
- Program ID is a placeholder; `anchor keys sync` rewrites it.
- This is a real, unrestricted power in production — the permanent delegate can
  drain any holder at any time. That's the point (and the controversy) of
  regulated tokens; a real issuer would put the admin behind a multisig.
- The permanent delegate is **immutable** once the mint is created — you cannot
  add or remove it later. That's why it must be set at mint init in the test.
