// ============================================================================
// P3b — Permanent Delegate Clawback (issuer recovers tokens)
//
// THE BIG IDEA: a regulated real-world asset sometimes MUST be taken back from a
// holder — a court order, sanctions, a compromised wallet, a failed KYC recheck.
// A normal SPL token makes this impossible: only the owner can move their
// tokens. Token-2022's PERMANENT DELEGATE extension is the escape hatch: a mint
// can name ONE pubkey that is permanently allowed to `transfer` or `burn` from
// ANY account of that mint — WITHOUT the holder's signature.
//
// How P3b differs from P3a (the two halves of "compliance"):
//   - P3a (transfer hook) answered: "WHO may RECEIVE this token?" — a rule
//     checked on every transfer, enforced by a separate program.
//   - P3b (permanent delegate) answers: "How does the issuer TAKE TOKENS BACK?"
//     — a mint-level extension naming an all-powerful authority.
//   Together they are the two sides of a regulated asset: gate who can hold it,
//   and let the issuer recover it.
//
// The design here (why a program at all, vs. just an admin keypair):
//   We make the permanent delegate a PDA of THIS program (seeds ["delegate"]).
//   Because the delegate is a program-derived address, only THIS program can
//   "sign" as it — via `with_signer(seeds)` in a CPI. So clawback can only
//   happen by calling our `clawback` instruction, which we gate with an admin
//   check (`has_one = admin`, exactly like P2's Config). That turns a raw,
//   dangerous power into a controlled, auditable one.
//
// Flow at runtime:
//   admin calls clawback(amount)
//      -> our program checks the caller is the stored admin
//      -> our program CPIs `transfer_checked` into Token-2022, with authority =
//         the delegate PDA, signed by the PDA's seeds
//      -> Token-2022 sees the mint's permanent delegate == that PDA, so it
//         allows the move FROM the victim account WITHOUT the victim signing
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, transfer_checked, Burn, Mint, TokenAccount, TokenInterface, TransferChecked,
};

declare_id!("ClawP3baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

#[program]
pub mod p3b_permanent_delegate {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_config
    //
    // Stores who is allowed to trigger clawbacks (the issuer's admin). A plain
    // program-owned data account, identical in spirit to P2's Config.
    // ------------------------------------------------------------------------
    pub fn initialize_config(ctx: Context<InitializeConfig>) -> Result<()> {
        ctx.accounts.config.admin = ctx.accounts.admin.key();
        msg!("Config initialized, admin = {}", ctx.accounts.config.admin);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: clawback
    //
    // The issuer force-moves `amount` tokens from ANY holder's account (`from`)
    // to a recovery account (`to`). The holder never signs. This works only
    // because the CPI is authorized by the delegate PDA, which is the mint's
    // permanent delegate.
    // ------------------------------------------------------------------------
    pub fn clawback(ctx: Context<Clawback>, amount: u64) -> Result<()> {
        // The delegate PDA "signs" the CPI with its seeds. Only this program can
        // produce this signature, so only this program (via the admin-gated
        // instruction) can exercise the permanent-delegate power.
        let bump = ctx.bumps.delegate;
        let signer_seeds: &[&[&[u8]]] = &[&[b"delegate", &[bump]]];

        // transfer_checked also verifies the mint + decimals match, so we can't
        // accidentally move the wrong token.
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.from.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.to.to_account_info(),
                    authority: ctx.accounts.delegate.to_account_info(), // the permanent delegate
                },
                signer_seeds,
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        msg!("Clawed back {} tokens from {}", amount, ctx.accounts.from.key());
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: burn_from
    //
    // The other permanent-delegate power: destroy tokens outright (e.g. sanctions
    // — the tokens must not just move, they must cease to exist). Same PDA-signed
    // CPI pattern, but into `burn` instead of `transfer_checked`.
    // ------------------------------------------------------------------------
    pub fn burn_from(ctx: Context<BurnFrom>, amount: u64) -> Result<()> {
        let bump = ctx.bumps.delegate;
        let signer_seeds: &[&[&[u8]]] = &[&[b"delegate", &[bump]]];

        burn(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.mint.to_account_info(),
                    from: ctx.accounts.from.to_account_info(),
                    authority: ctx.accounts.delegate.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        msg!("Burned {} tokens from {}", amount, ctx.accounts.from.key());
        Ok(())
    }
}

// ============================================================================
// STATE
// ============================================================================

#[account]
pub struct Config {
    pub admin: Pubkey, // who may trigger clawback / burn
}

impl Config {
    const SPACE: usize = 8 + 32; // discriminator + admin
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = Config::SPACE,
        seeds = [b"config"],
        bump
    )]
    pub config: Account<'info, Config>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Clawback<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"config"], bump, has_one = admin)]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub mint: InterfaceAccount<'info, Mint>,

    // The victim's account — force-debited. Note: NO signer for its owner.
    #[account(mut)]
    pub from: InterfaceAccount<'info, TokenAccount>,

    // Where recovered tokens land (the issuer's recovery account).
    #[account(mut)]
    pub to: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: the permanent-delegate PDA. Not a data account — just the address
    /// the mint names as its permanent delegate. We sign as it via its seeds.
    #[account(seeds = [b"delegate"], bump)]
    pub delegate: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct BurnFrom<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"config"], bump, has_one = admin)]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub from: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: the permanent-delegate PDA (see Clawback).
    #[account(seeds = [b"delegate"], bump)]
    pub delegate: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
