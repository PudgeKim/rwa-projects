// ============================================================================
// P7 — Capstone: a Full Compliant RWA Token
//
// This is the whole study plan on ONE token. A single Token-2022 mint stacks
// three extensions from earlier projects, and one program (one PDA) drives all
// of them:
//
//   Extension / feature            From   What it gives us
//   ----------------------------   -----  --------------------------------------
//   Default Account State = Frozen  P2    every new holder is FROZEN until KYC'd
//   Interest-Bearing                P5a   the share balance accrues yield
//   Permanent Delegate              P3b   the issuer can CLAW BACK shares
//   Pyth oracle pricing             P4    subscribe/redeem priced by SOL/USD
//   subscribe / redeem at NAV       P5b   deposit a volatile asset -> get shares
//
// THE ONE PDA TO RULE THEM ALL: seeds ["authority"] is simultaneously the mint
// authority (to issue shares), the freeze authority (to thaw = KYC), the
// permanent delegate (to claw back), and the vault owner (to custody deposits).
// So every privileged action is a CPI the program signs with that one PDA — the
// clean thread that ties the capstone together.
//
// THE COMPLIANCE STORY (watch this in the test):
//   1. A new user's share account is created FROZEN (default state). They cannot
//      receive shares yet — subscribe would fail (mint into a frozen account).
//   2. The admin calls verify_kyc, which THAWS that account. Now they're allowed.
//   3. subscribe mints them oracle-priced, yield-bearing shares.
//   4. If they must be removed (sanctions/court order), clawback force-moves
//      their shares to a recovery account — no signature from the holder.
//
// NOT in this program (by design, decoupled): the P3a transfer-hook allowlist is
// a SEPARATE program a mint points at; and trading happens on the P6 AMM. The
// README explains how both plug in, and how a venue (Kamino/Backpack/Kraken)
// would custody and list this token.
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, mint_to, thaw_account, transfer_checked, Burn, Mint, MintTo, ThawAccount, TokenAccount,
    TokenInterface, TransferChecked,
};
use pyth_solana_receiver_sdk::price_update::{get_feed_id_from_hex, Price, PriceUpdateV2};

declare_id!("Rwa7aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

// --- Oracle safety policy (from P4/P5b) --------------------------------------
const MAX_PRICE_AGE_SECS: u64 = 120;
const MAX_CONFIDENCE_BPS: u128 = 100;
const SOL_USD_FEED_ID: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

#[program]
pub mod p7_compliant_rwa {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_fund  (records mints, creates the deposit vault)
    // ------------------------------------------------------------------------
    pub fn initialize_fund(ctx: Context<InitializeFund>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.share_mint = ctx.accounts.share_mint.key();
        config.deposit_mint = ctx.accounts.deposit_mint.key();
        config.vault = ctx.accounts.vault.key();
        msg!("Compliant RWA fund initialized");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: verify_kyc  (P2)
    //
    // The compliance gate. The share mint's default account state is FROZEN, so
    // a new holder can't receive shares until the admin thaws their account.
    // Thawing = "this wallet passed KYC." The freeze authority is our PDA, so the
    // program signs the thaw CPI.
    // ------------------------------------------------------------------------
    pub fn verify_kyc(ctx: Context<VerifyKyc>) -> Result<()> {
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        thaw_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            ThawAccount {
                account: ctx.accounts.user_share_account.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
            signer_seeds,
        ))?;
        msg!("KYC verified: thawed {}", ctx.accounts.user_share_account.key());
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: subscribe  (P4 + P5b)
    //
    // Deposit a volatile asset, get oracle-priced, yield-bearing shares. This
    // FAILS if the user's share account is still frozen (not KYC'd) — the mint
    // CPI itself refuses to credit a frozen account. Compliance for free.
    // ------------------------------------------------------------------------
    pub fn subscribe(ctx: Context<Subscribe>, amount: u64) -> Result<()> {
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let shares = deposit_to_shares(
            amount,
            &price,
            ctx.accounts.deposit_mint.decimals,
            ctx.accounts.share_mint.decimals,
        )?;
        require!(shares > 0, RwaError::AmountTooSmall);

        // User pays the deposit into the vault (user signs).
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

        // Program mints shares (PDA signs). Reverts if the account is frozen.
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
            shares,
        )?;

        msg!("Subscribed: {} deposit -> {} shares", amount, shares);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 4: redeem  (P5b)  — burn shares, pay out the priced deposit.
    // ------------------------------------------------------------------------
    pub fn redeem(ctx: Context<Subscribe>, shares: u64) -> Result<()> {
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let deposit_out = shares_to_deposit(
            shares,
            &price,
            ctx.accounts.deposit_mint.decimals,
            ctx.accounts.share_mint.decimals,
        )?;
        require!(deposit_out > 0, RwaError::AmountTooSmall);

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
            deposit_out,
            ctx.accounts.deposit_mint.decimals,
        )?;

        msg!("Redeemed: {} shares -> {} deposit", shares, deposit_out);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 5: clawback  (P3b)
    //
    // The issuer force-moves shares from any holder to a recovery account, with
    // no holder signature — authorized because our PDA is the mint's permanent
    // delegate. Admin-gated.
    // ------------------------------------------------------------------------
    pub fn clawback(ctx: Context<Clawback>, amount: u64) -> Result<()> {
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.from.to_account_info(),
                    mint: ctx.accounts.share_mint.to_account_info(),
                    to: ctx.accounts.to.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(), // permanent delegate
                },
                signer_seeds,
            ),
            amount,
            ctx.accounts.share_mint.decimals,
        )?;
        msg!("Clawed back {} shares from {}", amount, ctx.accounts.from.key());
        Ok(())
    }
}

// ============================================================================
// PRICING (from P5b)
// ============================================================================

fn load_validated_price(price_update: &Account<PriceUpdateV2>) -> Result<Price> {
    let feed_id = get_feed_id_from_hex(SOL_USD_FEED_ID)?;
    let price =
        price_update.get_price_no_older_than(&Clock::get()?, MAX_PRICE_AGE_SECS, &feed_id)?;
    require!(price.price > 0, RwaError::NonPositivePrice);
    let conf_bps = (price.conf as u128)
        .checked_mul(10_000)
        .and_then(|v| v.checked_div(price.price as u128))
        .ok_or(RwaError::MathOverflow)?;
    require!(conf_bps <= MAX_CONFIDENCE_BPS, RwaError::PriceTooUncertain);
    Ok(price)
}

fn deposit_to_shares(
    deposit_amount: u64,
    price: &Price,
    deposit_decimals: u8,
    share_decimals: u8,
) -> Result<u64> {
    let shift = price.exponent + share_decimals as i32 - deposit_decimals as i32;
    let base = (deposit_amount as u128)
        .checked_mul(price.price as u128)
        .ok_or(RwaError::MathOverflow)?;
    u64::try_from(apply_pow10(base, shift)?).map_err(|_| RwaError::MathOverflow.into())
}

fn shares_to_deposit(
    shares: u64,
    price: &Price,
    deposit_decimals: u8,
    share_decimals: u8,
) -> Result<u64> {
    let shift = deposit_decimals as i32 - share_decimals as i32 - price.exponent;
    let numerator = apply_pow10(shares as u128, shift)?;
    let out = numerator
        .checked_div(price.price as u128)
        .ok_or(RwaError::MathOverflow)?;
    u64::try_from(out).map_err(|_| RwaError::MathOverflow.into())
}

fn apply_pow10(value: u128, shift: i32) -> Result<u128> {
    if shift >= 0 {
        let f = 10u128.checked_pow(shift as u32).ok_or(RwaError::MathOverflow)?;
        value.checked_mul(f).ok_or(RwaError::MathOverflow.into())
    } else {
        let f = 10u128.checked_pow((-shift) as u32).ok_or(RwaError::MathOverflow)?;
        Ok(value / f)
    }
}

// ============================================================================
// STATE
// ============================================================================

#[account]
pub struct Config {
    pub admin: Pubkey,
    pub share_mint: Pubkey,
    pub deposit_mint: Pubkey,
    pub vault: Pubkey,
}

impl Config {
    const SPACE: usize = 8 + 32 * 4;
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializeFund<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(init, payer = admin, space = Config::SPACE, seeds = [b"config"], bump)]
    pub config: Account<'info, Config>,

    // The compliant share mint (created in the test with the 3 extensions;
    // mint + freeze authority + permanent delegate all = the `authority` PDA).
    pub share_mint: InterfaceAccount<'info, Mint>,

    // The volatile deposit mint (mock wSOL).
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: the all-powers PDA (mint auth, freeze auth, permanent delegate,
    /// vault owner). Just an address the program signs for.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(
        init, payer = admin, seeds = [b"vault"], bump,
        token::mint = deposit_mint, token::authority = authority, token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VerifyKyc<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"config"], bump, has_one = admin, has_one = share_mint)]
    pub config: Account<'info, Config>,

    pub share_mint: InterfaceAccount<'info, Mint>,

    // The holder's share account to thaw (mark as KYC-approved).
    #[account(mut)]
    pub user_share_account: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: freeze-authority PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

// subscribe and redeem share this context.
#[derive(Accounts)]
pub struct Subscribe<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"config"], bump,
        has_one = share_mint, has_one = deposit_mint, has_one = vault,
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

    /// CHECK: mint-authority / vault-owner PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub price_update: Account<'info, PriceUpdateV2>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Clawback<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"config"], bump, has_one = admin, has_one = share_mint)]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub from: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub to: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: permanent-delegate PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum RwaError {
    #[msg("Price confidence band is too wide to trust")]
    PriceTooUncertain,
    #[msg("Price is zero or negative")]
    NonPositivePrice,
    #[msg("Amount too small — rounds to zero")]
    AmountTooSmall,
    #[msg("Arithmetic overflow while pricing")]
    MathOverflow,
}
