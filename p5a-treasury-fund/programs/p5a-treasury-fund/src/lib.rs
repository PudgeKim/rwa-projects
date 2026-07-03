// ============================================================================
// P5a — Yield-bearing Tokenized Treasury (fund shares, fixed $1 NAV)
//
// THE BIG IDEA: a tokenized treasury (think Ondo USDY, Superstate USTB,
// BlackRock BUIDL) is a token that represents a SHARE of a fund holding real
// T-bills. Two things make it special:
//   1. It ACCRUES YIELD — a share is worth a little more each day.
//   2. You SUBSCRIBE (deposit cash -> get shares) and REDEEM (shares -> cash)
//      at the fund's NAV (net asset value per share).
//
// P5a builds the SUBSCRIBE/REDEEM machinery with a FIXED NAV of $1/share, and
// represents yield with Token-2022's INTEREST-BEARING extension. P5b will swap
// the fixed price for a live Pyth oracle (reusing P4).
//
// Where the yield comes from — the key Token-2022 lesson:
//   The share mint is created (in the test) with the Interest-Bearing extension
//   and a rate. That extension does NOT mint new tokens. It only changes the
//   *displayed* balance: `uiAmount = rawAmount grown by the rate over time`.
//   So a holder's RAW amount is constant, but their UI (spendable-value) amount
//   creeps up — that IS the yield. Our program only ever deals in RAW amounts
//   (mint/burn/transfer), which is exactly how a real fund accounts for shares.
//
// How P5a differs from P1–P4:
//   - P2/P3 were compliance on a single token. P5a has TWO tokens and moves
//     value between them: a DEPOSIT token (cash, e.g. mock USDC) and a SHARE
//     token (the fund). Subscribe swaps cash -> shares; redeem swaps back.
//   - It introduces a VAULT: a program-owned token account that custodies the
//     deposited cash, and a PDA that is both the vault owner and the share
//     mint authority. One PDA signs for everything the program does.
//
// Flow at runtime:
//   subscribe(amount):  user's cash --transfer--> vault
//                       shares      --mint(PDA)--> user     (shares = amount, $1 NAV)
//   redeem(shares):     user's shares --burn-->   gone
//                       cash        --transfer(PDA)--> user (cash = shares, $1 NAV)
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, mint_to, transfer_checked, Burn, Mint, MintTo, TokenAccount, TokenInterface,
    TransferChecked,
};

declare_id!("Fund5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

#[program]
pub mod p5a_treasury_fund {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_fund
    //
    // Records the two mints (cash + shares) and creates the vault that will
    // custody deposited cash. The vault is owned by the program's `authority`
    // PDA — the same PDA that is the share mint's authority (set in the test).
    // ------------------------------------------------------------------------
    pub fn initialize_fund(ctx: Context<InitializeFund>) -> Result<()> {
        // NAV is fixed at $1 here, which means shares and cash trade 1:1 — but
        // only if they use the SAME decimals, so the raw integer amounts line up.
        require!(
            ctx.accounts.share_mint.decimals == ctx.accounts.deposit_mint.decimals,
            FundError::DecimalMismatch
        );

        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.share_mint = ctx.accounts.share_mint.key();
        config.deposit_mint = ctx.accounts.deposit_mint.key();
        config.vault = ctx.accounts.vault.key();
        msg!("Fund initialized (NAV fixed at $1/share)");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: subscribe
    //
    // User deposits `amount` of cash into the vault and receives `amount` fund
    // shares (1:1, because NAV is $1 and decimals match). Two CPIs:
    //   (a) user signs a transfer of cash -> vault
    //   (b) the PDA signs a mint of shares -> user
    // ------------------------------------------------------------------------
    pub fn subscribe(ctx: Context<Subscribe>, amount: u64) -> Result<()> {
        // (a) Pull the cash from the user into the vault. The USER is the
        // authority here, so the user must sign this transaction.
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.user_deposit_ata.to_account_info(),
                    mint: ctx.accounts.deposit_mint.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.deposit_mint.decimals,
        )?;

        // (b) Mint the matching shares to the user. The mint authority is our
        // PDA, so the PROGRAM signs with the PDA's seeds (the user cannot mint).
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.share_mint.to_account_info(),
                    to: ctx.accounts.user_share_ata.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer_seeds,
            ),
            amount, // shares = amount at $1 NAV
        )?;

        msg!("Subscribed: {} cash in, {} shares minted", amount, amount);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: redeem
    //
    // The reverse: burn `shares` from the user and pay out the same amount of
    // cash from the vault. Two CPIs:
    //   (a) user signs a burn of their shares
    //   (b) the PDA signs a transfer of cash vault -> user
    // ------------------------------------------------------------------------
    pub fn redeem(ctx: Context<Redeem>, shares: u64) -> Result<()> {
        // (a) Burn the user's shares. The user is the authority — they sign.
        burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.share_mint.to_account_info(),
                    from: ctx.accounts.user_share_ata.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            shares,
        )?;

        // (b) Pay cash out of the vault. The vault's owner is our PDA, so the
        // PROGRAM signs this transfer with the PDA's seeds.
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.deposit_mint.to_account_info(),
                    to: ctx.accounts.user_deposit_ata.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer_seeds,
            ),
            shares, // cash out = shares at $1 NAV
            ctx.accounts.deposit_mint.decimals,
        )?;

        msg!("Redeemed: {} shares burned, {} cash out", shares, shares);
        Ok(())
    }
}

// ============================================================================
// STATE
// ============================================================================

#[account]
pub struct Config {
    pub admin: Pubkey,        // who set the fund up
    pub share_mint: Pubkey,   // the interest-bearing fund token
    pub deposit_mint: Pubkey, // the cash token (mock USDC)
    pub vault: Pubkey,        // program-owned account custodying deposited cash
}

impl Config {
    const SPACE: usize = 8 + 32 + 32 + 32 + 32;
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializeFund<'info> {
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

    // The fund share mint, created in the test WITH the interest-bearing
    // extension and mint authority = the `authority` PDA below.
    pub share_mint: InterfaceAccount<'info, Mint>,

    // The cash mint (mock USDC), created in the test.
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA that owns the vault and is the share mint authority. Just an
    /// address the program signs for; not a data account.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    // The vault: a program-owned token account for the cash mint, owned by the
    // authority PDA. Anchor allocates and initializes it here.
    #[account(
        init,
        payer = admin,
        seeds = [b"vault"],
        bump,
        token::mint = deposit_mint,
        token::authority = authority,
        token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Subscribe<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    // has_one checks pin the passed mints/vault to what the fund was set up with,
    // so a caller can't sneak in a different share mint or a fake vault.
    #[account(
        seeds = [b"config"],
        bump,
        has_one = share_mint,
        has_one = deposit_mint,
        has_one = vault,
    )]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub share_mint: InterfaceAccount<'info, Mint>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_deposit_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_share_ata: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: the mint-authority / vault-owner PDA (see InitializeFund).
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Redeem<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"config"],
        bump,
        has_one = share_mint,
        has_one = deposit_mint,
        has_one = vault,
    )]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub share_mint: InterfaceAccount<'info, Mint>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_deposit_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_share_ata: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: the mint-authority / vault-owner PDA (see InitializeFund).
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum FundError {
    #[msg("Share and deposit mints must use the same decimals for a 1:1 NAV")]
    DecimalMismatch,
}
