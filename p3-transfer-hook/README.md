# P3a — Compliance Transfer Hook (allowlist)

The third mini-project in the [RWA study plan](../../.claude/study-plan.md).
Goal: enforce a rule on **every single transfer**. A Token-2022 mint names a
**separate program** (a *transfer hook*) that Token-2022 calls on each transfer;
if it errors, the transfer reverts. Here the rule is an **allowlist** — the
receiver's wallet must be approved, or the transfer is rejected.

> P3 is split: **P3a = transfer hook + allowlist (this project)**. **P3b** will add
> Permanent Delegate **clawback** next.

## The key mental shift vs P2

| | P2 (KYC freeze) | P3 (transfer hook) |
|---|---|---|
| Who owns the token | **our** program (mint/freeze authority) | the issuer; our program does **not** own it |
| What our program is | the token's controller | a reusable **rule-checker** Token-2022 calls into |
| Granularity | freeze/thaw a **whole account** | a check on **every transfer** |
| Who calls our code | the admin (our instructions) | **Token-2022**, via CPI, automatically |

So the hook program is **decoupled** from token issuance: many mints could point
at this one hook. That's why the mint is created in the **test/client**, not in
the program — the program is just the enforcer.

## What the program does (`programs/p3-transfer-hook/src/lib.rs`)

1. `initialize_white_list` — create the global allowlist (stores `authority` + a
   `Vec<Pubkey>` of allowed receivers). A normal data account, like P2's `Config`.
2. `add_to_white_list` — the authority adds an allowed wallet (`has_one = authority`).
3. `initialize_extra_account_meta_list` — **per mint**. Creates the
   `ExtraAccountMetaList` PDA (fixed seeds `["extra-account-metas", mint]`) that
   tells Token-2022 which extra accounts to pass our hook — here, the white-list.
   Built with a **raw `create_account` CPI** (dynamic size), reusing P2's pattern.
4. `transfer_hook` — the `Execute` interface Token-2022 CPIs into on every
   transfer. Checks the **destination account's owner** is on the white-list;
   otherwise returns an error and the whole transfer reverts.

## Reading guide

Read `lib.rs` in this order:
1. Top comment — the runtime flow (who calls the hook, and when).
2. `transfer_hook` (the `Execute`) — start at the **rule** itself, then see what
   accounts it receives and in what fixed order.
3. `initialize_extra_account_meta_list` — how Token-2022 learns which extra
   accounts to pass (the `ExtraAccountMeta` + `Seed::Literal` for the white-list).
4. `WhiteList` state + `initialize_white_list` / `add_to_white_list`.
5. `tests/p3-transfer-hook.ts` — the mint is created with
   `createInitializeTransferHookInstruction` pointing at our program; the
   transfer to a non-allowlisted wallet **fails**, then **succeeds** after adding it.

Good questions to bring: *Why is the mint created in the test, not the program?
What is the ExtraAccountMetaList for — why can't Token-2022 just guess the extra
accounts? Why must the meta-list PDA seeds be exactly `["extra-account-metas",
mint]`? What does `#[interface(...execute)]` do? Why check the **destination**
owner and not the source? Could someone call `transfer_hook` directly to spoof a
success?*

## Build & run

Toolchain (one-time, Homebrew — same as P1/P2):
```bash
! brew install solana anchor
! cd /Users/kanetla/Documents/rwa/projects/p3-transfer-hook && npm install
! solana-keygen new --no-bip39-passphrase   # if you don't already have a wallet
```

Build + test:
```bash
! cd /Users/kanetla/Documents/rwa/projects/p3-transfer-hook && anchor keys sync && anchor build && anchor test
```
Success = `6 passing`.

## ⚠️ Heads-up: transfer hooks are version-sensitive

This is the most fragile corner of the Solana toolchain. The two crates
`spl-transfer-hook-interface` and `spl-tlv-account-resolution` (in the program's
`Cargo.toml`) **must agree** with the `spl-token-2022` version that `anchor-spl
1.0.2` pulls in. If the **first `anchor build`** fails with errors about
duplicate / incompatible `solana-program` or `spl-*` versions, that's expected —
**paste me the error** and we'll align the versions (often a one-line pin or a
`[patch]`). The *structure* of the code won't change; only the version numbers.

## Notes
- Program ID is a placeholder; `anchor keys sync` rewrites it. The mint points at
  the hook by program id, and the test reads `program.programId` dynamically, so
  the sync is enough.
- The allowlist is a single global account pre-sized for `MAX_WALLETS = 10`
  (no `realloc`). Bump the constant for more.
- **Anti-spoofing (advanced, not implemented here):** because anyone can *call*
  `transfer_hook` directly, production hooks also assert the token accounts are in
  the Token-2022 "transferring" state (set only during a real transfer CPI). We
  skip that for readability — a good follow-up question.
