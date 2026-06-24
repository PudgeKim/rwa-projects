# P1 — Bare SPL Token (mint + transfer)

The first mini-project in the [RWA study plan](../../.claude/study-plan.md).
Goal: learn the four Anchor fundamentals everything else builds on —
**Accounts, PDAs, CPI, Rent** — using a plain SPL token (no compliance yet).

## What this program does

Three instructions (see `programs/p1-spl-token/src/lib.rs`):

1. `initialize_mint` — create the token mint (6 decimals). The mint is a **PDA**,
   so the program itself controls minting.
2. `mint_tokens` — mint new units into a recipient's Associated Token Account.
   Demonstrates **CPI + PDA signing** (the program signs as the mint authority).
3. `transfer_tokens` — move units between two wallets' token accounts.
   Demonstrates **CPI with a normal wallet signer** (no PDA signing).

## Reading guide (do this first — it's the point of the method)

Read in this order and ask me about anything unclear:

1. `programs/p1-spl-token/src/lib.rs` — top comment, then the three instructions,
   then the `#[derive(Accounts)]` structs. The comments explain each line.
2. `tests/p1-spl-token.ts` — how a client calls each instruction.

Good questions to bring me: *What is a PDA "bump"? Why does the program sign
instead of me? What is an ATA? Why `transfer_checked` instead of `transfer`?
Why are mint authority and freeze authority separate?*

**Q&A study notes** (questions already worked through, with answers):
`~/Documents/notes/life/0. Now/RWA/Mini-projects/P1-bare-spl-token-QnA.md`

## One-time toolchain install

Rust + Node are already on this machine. You still need the Solana CLI and Anchor —
both are in Homebrew, so install them that way:

```bash
! brew install solana anchor

# Test deps for this project
! cd projects/p1-spl-token && npm install
```

Verify: `! solana --version && anchor --version` (expect Solana 4.x, Anchor 1.0.x).

## Build, test, run

```bash
# from projects/p1-spl-token/

# Make the program's on-chain ID match a real local keypair
! anchor keys sync

# Compile the Rust program to BPF bytecode
! anchor build

# Build + spin up a local validator + deploy + run the TypeScript tests
! anchor test
```

`anchor test` is the fastest feedback loop — it runs everything in `tests/`.

## Notes
- Program ID `3pX5...TSn8d` in `lib.rs` / `Anchor.toml` is a placeholder;
  `anchor keys sync` rewrites it to the keypair Anchor generates locally.
- Crate versions are pinned to Anchor `1.0.2` (matches the Homebrew formula).
  If your installed `anchor --version` differs and the build complains, tell me
  and we'll align them.
