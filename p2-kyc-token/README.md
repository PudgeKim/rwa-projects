# P2 — KYC-gated token (Token-2022 `Default Account State`)

The second mini-project in the [RWA study plan](../../.claude/study-plan.md).
Goal: build a **permissioned token** — every new holder's account is **frozen by
default** and only works after an admin **verifies KYC** (thaws it). This is the
foundation of regulated / RWA tokens.

## What's new vs P1

| Concept | P1 (classic SPL) | P2 (Token-2022) |
|---|---|---|
| Token program | Token (`Tokenkeg…`) | **Token-2022** (`Tokenz…`) |
| Special features | none | **extensions** — here `DefaultAccountState` |
| Mint creation | Anchor `init` constraint | **raw CPIs** (extension needs manual setup) |
| Program state | none | a **`Config`** data account (+ `has_one` auth) |
| Freeze/thaw | — | `freeze_account` / `thaw_account` (PDA-signed) |

## What the program does (`programs/p2-kyc-token/src/lib.rs`)

1. `initialize` — create the `Config` (stores admin) **and** a Token-2022 mint
   whose accounts are **frozen by default**. The mint is built with three raw
   CPIs in order: `create_account` → `default_account_state_initialize` →
   `initialize_mint2`. The Config PDA is the mint + freeze authority.
2. `create_user_token_account` — make a user's ATA (born **frozen**).
3. `verify_kyc` — admin **thaws** the user's account (PDA-signed CPI).
4. `revoke_kyc` — admin **re-freezes** it.
5. `mint_to_user` — mint tokens; **fails while frozen**, succeeds after KYC.

## Reading guide

Read `lib.rs` in this order:
1. Top comment (the 4 new concepts).
2. `initialize` — the **3-step raw-CPI mint creation**. Key idea: an extension
   must be initialized **before** `initialize_mint2`.
3. `Config` struct + the `has_one = admin` constraint (authorization).
4. `verify_kyc` / `revoke_kyc` / `mint_to_user` — `with_signer` PDA authority
   (same mechanism as P1's mint, now the Config PDA).
5. `tests/p2-kyc-token.ts` — watch the gate: mint-to-frozen **fails**, thaw,
   mint **succeeds**, revoke, **fails** again.

Good questions to bring: *Why must the extension be set before `initialize_mint2`?
Why is the mint a `Signer` here but a PDA in P1? What does `has_one` check? Why
does minting to a frozen account fail? Who could thaw without the admin?*

## Build & run

Toolchain (one-time, Homebrew — same as P1):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p2-kyc-token && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p2-kyc-token && anchor keys sync && anchor build && anchor test
```
Success = `5 passing`.

## Notes
- Program ID is a placeholder; `anchor keys sync` rewrites it.
- Pinned to Anchor `1.0.2`. The extension helpers come from
  `anchor-spl` with the `token_2022_extensions` feature. If a build error
  mentions a missing extension function/feature, tell me and we'll adjust.
