// ============================================================================
// P1 — Bare SPL Token (mint + transfer)
//
// Goal of this mini-project: learn the four Anchor fundamentals that EVERY
// later RWA project depends on:
//   1. Accounts      — Solana stores all state in "accounts"; programs are stateless.
//   2. PDAs          — Program Derived Addresses: accounts a program controls.
//   3. CPI           — Cross-Program Invocation: calling another program.
//   4. Rent          — paying SOL to keep an account alive on-chain.
//
// We do NOT use any compliance features here. This is the "plain token" you must
// understand before Token-2022 (P2/P3) makes sense.
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    // The "classic" SPL Token program helpers. In P2 we switch to
    // `token_interface` to support Token-2022 — for now we use the original.
    token::{mint_to, transfer_checked, Mint, MintTo, Token, TokenAccount, TransferChecked},
};

// Every Solana program has an on-chain address. `declare_id!` records it inside
// the program so the program can verify it is the one being called.
// After you install Anchor, run `anchor keys sync` and this value is replaced
// with the real keypair Anchor generated for you.
declare_id!("3pX5NKLru1UBDVckynWQxsgnJeUN3N1viy36Gk9TSn8d");

// The `#[program]` module holds the program's INSTRUCTIONS — the entry points
// a client can call. Each public function is one instruction.
#[program]
pub mod p1_spl_token {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_mint
    //
    // A "mint" is the account that defines a token: its decimals, its supply,
    // and who is allowed to mint new units (the mint authority).
    //
    // All the real work happens in the `InitializeMint` accounts struct below
    // via Anchor's `init` constraint. The body is empty because Anchor already
    // created and configured the mint before this function ran.
    // ------------------------------------------------------------------------
    pub fn initialize_mint(_ctx: Context<InitializeMint>) -> Result<()> {
        msg!("Mint initialized — decimals=6, authority = the mint PDA itself");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: mint_tokens
    //
    // Creates `amount` brand-new token units and deposits them into a
    // recipient's token account. Only the mint authority may do this.
    //
    // KEY IDEA: our mint authority is the mint PDA itself. A PDA has no private
    // key, so a human can't sign for it. Instead the PROGRAM signs on the PDA's
    // behalf by providing the PDA's seeds + bump — this is `with_signer`.
    // This is your first taste of CPI (calling the Token program) + PDA signing.
    // ------------------------------------------------------------------------
    pub fn mint_tokens(ctx: Context<MintTokens>, amount: u64) -> Result<()> {
        // The seeds that derive the mint PDA, plus the bump Anchor found for us.
        // This array is how the program "proves" it controls the PDA.
        let signer_seeds: &[&[&[u8]]] = &[&[b"mint", &[ctx.bumps.mint]]];

        // Describe the accounts the Token program's `mint_to` instruction needs.
        let cpi_accounts = MintTo {
            mint: ctx.accounts.mint.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            // authority == the mint PDA, which is why we must sign with seeds.
            authority: ctx.accounts.mint.to_account_info(),
        };

        // A CpiContext bundles (which program to call) + (which accounts) and,
        // here, the PDA signer seeds so the call is authorized.
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        )
        .with_signer(signer_seeds);

        // Make the cross-program call. `?` bubbles up any error.
        mint_to(cpi_ctx, amount)?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: transfer_tokens
    //
    // Moves `amount` units from the sender's token account to a recipient's.
    // Here the authority is a real wallet (the `sender` signer), so NO PDA
    // signing is needed — the human's signature authorizes the transfer.
    //
    // We use `transfer_checked` (not the old `transfer`) because it also
    // verifies the mint and decimals, preventing a class of bugs.
    // ------------------------------------------------------------------------
    pub fn transfer_tokens(ctx: Context<TransferTokens>, amount: u64) -> Result<()> {
        let decimals = ctx.accounts.mint.decimals;

        let cpi_accounts = TransferChecked {
            mint: ctx.accounts.mint.to_account_info(),
            from: ctx.accounts.sender_token_account.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.sender.to_account_info(),
        };

        // No `.with_signer(...)` — the `sender` already signed the transaction.
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        );

        transfer_checked(cpi_ctx, amount, decimals)?;
        Ok(())
    }
}

// ============================================================================
// ACCOUNTS STRUCTS
//
// On Solana a program does not "have" state — the caller must pass in EVERY
// account the instruction will touch. These `#[derive(Accounts)]` structs
// declare exactly which accounts are required and the rules ("constraints")
// they must satisfy. Anchor checks all of this BEFORE your function runs.
// ============================================================================

#[derive(Accounts)]
pub struct InitializeMint<'info> {
    // `mut` because the payer's SOL balance changes (they pay rent + fees).
    // `Signer` means this account must have signed the transaction.
    #[account(mut)]
    pub payer: Signer<'info>,

    // The mint we are creating, as a PDA so the PROGRAM controls it.
    #[account(
        init,                       // create this account
        payer = payer,              // payer funds the rent
        seeds = [b"mint"],          // PDA seeds -> deterministic address
        bump,                       // Anchor finds the canonical bump
        mint::decimals = 6,         // 1 token = 1_000_000 base units
        mint::authority = mint,     // the mint PDA is its own mint authority
    )]
    pub mint: Account<'info, Mint>,

    // Programs we will call into / that Anchor needs to create accounts.
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct MintTokens<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    // Must already exist; `mut` because supply will increase.
    #[account(mut, seeds = [b"mint"], bump)]
    pub mint: Account<'info, Mint>,

    // The wallet that will OWN the tokens. We never read its data, so it's
    // Unchecked. `/// CHECK:` documents why that's safe (it's just a key).
    /// CHECK: only used as the owner/authority of the recipient ATA.
    pub recipient: UncheckedAccount<'info>,

    // The recipient's Associated Token Account (ATA): the canonical token
    // account for (this mint, this owner). `init_if_needed` creates it the
    // first time and reuses it afterward.
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = mint,
        associated_token::authority = recipient,
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TransferTokens<'info> {
    // The sender signs, so they authorize moving their own tokens.
    #[account(mut)]
    pub sender: Signer<'info>,

    pub mint: Account<'info, Mint>,

    // Sender's ATA — must exist and be owned by `sender`. `mut` (balance drops).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = sender,
    )]
    pub sender_token_account: Account<'info, TokenAccount>,

    /// CHECK: only used as the owner/authority of the recipient ATA.
    pub recipient: UncheckedAccount<'info>,

    // Create the recipient's ATA on the fly if it doesn't exist yet.
    #[account(
        init_if_needed,
        payer = sender,
        associated_token::mint = mint,
        associated_token::authority = recipient,
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}
