// ============================================================================
// P5b — Tokenized Treasury with ORACLE-PRICED NAV
//
// THE BIG IDEA: P5a subscribed/redeemed at a fixed 1:1 rate because the deposit
// was cash ($1 = $1). Real funds take deposits in assets whose price MOVES, so
// the number of shares you get must be computed from the deposit's *USD value*:
//
//     shares = USD_value_of_deposit / NAV_per_share
//
// P5b makes the deposit a VOLATILE asset (a mock wSOL) and gets its USD price
// from Pyth — reusing P4's exact safety checks (staleness, confidence, feed id).
// NAV per share stays $1 (that part is still simple); what's new is that the
// DEPOSIT is now priced by an oracle instead of assumed to be a dollar.
//
// How P5b differs from P5a:
//   - P5a: deposit and shares had equal decimals and traded 1:1.
//   - P5b: deposit (wSOL, 9 decimals) and shares (6 decimals) differ, and the
//     exchange rate is `price * decimal-scaling`. All the interesting code is
//     that conversion — see `deposit_to_shares` / `shares_to_deposit`.
//   - Every subscribe/redeem now takes a Pyth `PriceUpdateV2` account and
//     validates it before trusting the number (this is P4, embedded).
//
// Flow at runtime:
//   subscribe(amount wSOL):  read+validate SOL/USD  ->  shares = wSOL*price/$1
//                            wSOL --transfer--> vault ; shares --mint(PDA)--> user
//   redeem(shares):          read+validate SOL/USD  ->  wSOL = shares*$1/price
//                            shares --burn--> gone   ; wSOL  --transfer(PDA)--> user
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, mint_to, transfer_checked, Burn, Mint, MintTo, TokenAccount, TokenInterface,
    TransferChecked,
};
use pyth_solana_receiver_sdk::price_update::{get_feed_id_from_hex, Price, PriceUpdateV2};

declare_id!("Fund5baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

// --- Oracle safety policy (identical to P4) ----------------------------------
const MAX_PRICE_AGE_SECS: u64 = 120;
const MAX_CONFIDENCE_BPS: u128 = 100; // reject a band wider than 1% of the price
const SOL_USD_FEED_ID: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

#[program]
pub mod p5b_treasury_oracle {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_fund
    //
    // Same as P5a, but WITHOUT the equal-decimals requirement — deposit and
    // shares are now different assets, bridged by the oracle price.
    // ------------------------------------------------------------------------
    pub fn initialize_fund(ctx: Context<InitializeFund>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.share_mint = ctx.accounts.share_mint.key();
        config.deposit_mint = ctx.accounts.deposit_mint.key();
        config.vault = ctx.accounts.vault.key();
        msg!("Fund initialized (oracle-priced deposits, NAV $1/share)");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: subscribe
    //
    // User deposits `amount` of the volatile asset; we price it in USD and mint
    // that many dollars' worth of shares.
    // ------------------------------------------------------------------------
    pub fn subscribe(ctx: Context<Subscribe>, amount: u64) -> Result<()> {
        // Price the deposit (P4's checks: fresh, tight, correct feed).
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let shares = deposit_to_shares(
            amount,
            &price,
            ctx.accounts.deposit_mint.decimals,
            ctx.accounts.share_mint.decimals,
        )?;
        require!(shares > 0, FundError::AmountTooSmall);

        // (a) User signs the transfer of their volatile asset into the vault.
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

        // (b) The PDA signs the mint of `shares` (USD value at $1 NAV) to the user.
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

        msg!("Subscribed: {} deposit units -> {} shares (~USD)", amount, shares);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: redeem
    //
    // The reverse: burn `shares`, price them in USD, and pay out that USD value
    // in the volatile asset at the CURRENT price.
    // ------------------------------------------------------------------------
    pub fn redeem(ctx: Context<Redeem>, shares: u64) -> Result<()> {
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let deposit_out = shares_to_deposit(
            shares,
            &price,
            ctx.accounts.deposit_mint.decimals,
            ctx.accounts.share_mint.decimals,
        )?;
        require!(deposit_out > 0, FundError::AmountTooSmall);

        // (a) User signs the burn of their shares.
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

        // (b) The PDA signs the payout of the volatile asset from the vault.
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

        msg!("Redeemed: {} shares -> {} deposit units", shares, deposit_out);
        Ok(())
    }
}

// ============================================================================
// PRICING MATH  (the heart of P5b)
// ============================================================================

// Validate a Pyth price exactly like P4: recent, tight confidence, right feed.
fn load_validated_price(price_update: &Account<PriceUpdateV2>) -> Result<Price> {
    let feed_id = get_feed_id_from_hex(SOL_USD_FEED_ID)?;
    let price =
        price_update.get_price_no_older_than(&Clock::get()?, MAX_PRICE_AGE_SECS, &feed_id)?;

    require!(price.price > 0, FundError::NonPositivePrice);
    let conf_bps = (price.conf as u128)
        .checked_mul(10_000)
        .and_then(|v| v.checked_div(price.price as u128))
        .ok_or(FundError::MathOverflow)?;
    require!(conf_bps <= MAX_CONFIDENCE_BPS, FundError::PriceTooUncertain);
    Ok(price)
}

// shares = deposit_amount * price, adjusted for the two tokens' decimals.
//
// A Pyth price is `mantissa * 10^exponent` USD per WHOLE deposit token. Working
// in raw base units and $1 NAV (so shares are just USD scaled to share decimals):
//   shares_raw = deposit_amount * mantissa * 10^(exponent + share_dec - deposit_dec)
fn deposit_to_shares(
    deposit_amount: u64,
    price: &Price,
    deposit_decimals: u8,
    share_decimals: u8,
) -> Result<u64> {
    let shift = price.exponent + share_decimals as i32 - deposit_decimals as i32;
    let base = (deposit_amount as u128)
        .checked_mul(price.price as u128)
        .ok_or(FundError::MathOverflow)?;
    let scaled = apply_pow10(base, shift)?;
    u64::try_from(scaled).map_err(|_| FundError::MathOverflow.into())
}

// deposit = shares / price — the exact inverse of the above:
//   deposit_raw = shares * 10^(deposit_dec - share_dec - exponent) / mantissa
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
        .ok_or(FundError::MathOverflow)?;
    u64::try_from(out).map_err(|_| FundError::MathOverflow.into())
}

// Multiply by 10^shift when shift >= 0, divide (floor) when shift < 0.
fn apply_pow10(value: u128, shift: i32) -> Result<u128> {
    if shift >= 0 {
        let factor = 10u128
            .checked_pow(shift as u32)
            .ok_or(FundError::MathOverflow)?;
        value.checked_mul(factor).ok_or(FundError::MathOverflow.into())
    } else {
        let factor = 10u128
            .checked_pow((-shift) as u32)
            .ok_or(FundError::MathOverflow)?;
        Ok(value / factor)
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

    // Interest-bearing fund share mint (created in the test; authority = PDA).
    pub share_mint: InterfaceAccount<'info, Mint>,

    // The volatile deposit mint (mock wSOL).
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA that owns the vault and is the share mint authority.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

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

    /// CHECK: the mint-authority / vault-owner PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    // The Pyth price update, posted by the client. Typed as PriceUpdateV2 so
    // Anchor verifies it is owned by the Pyth receiver program (see P4).
    pub price_update: Account<'info, PriceUpdateV2>,

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

    /// CHECK: the mint-authority / vault-owner PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub price_update: Account<'info, PriceUpdateV2>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum FundError {
    #[msg("Price confidence band is too wide to trust")]
    PriceTooUncertain,
    #[msg("Price is zero or negative")]
    NonPositivePrice,
    #[msg("Amount too small — rounds to zero shares/deposit at this price")]
    AmountTooSmall,
    #[msg("Arithmetic overflow while pricing")]
    MathOverflow,
}
