// ============================================================================
// P6a — Tokenized Stock Issuance (primary market, oracle-priced)
//
// THE BIG IDEA: a tokenized stock (think xStocks' AAPLx, or Backed's bCSPX) is a
// token where 1 token == 1 share of a real stock, held in custody. The ISSUER
// runs the primary market: you BUY shares by paying USDC at the current share
// price, and SELL them back for USDC — both priced by an oracle.
//
// This is the "primary market". P6b adds the "secondary market": a mini AMM
// where the token trades freely and its price is discovered by supply/demand.
// The two prices are kept in line by arbitrage — the key xStocks insight.
//
// How P6a relates to P5b: the machinery (oracle + vault + mint/burn) is the same
// family as the treasury fund, but the DIRECTION flips. In P5b you deposited a
// volatile asset and got $1-NAV shares. Here you specify how many SHARES you
// want and pay the oracle-priced USDC cost:
//     cost_usdc = shares * price_per_share
//
// NOTE ON THE FEED: a real deployment uses an equity feed (e.g. AAPL/USD). Those
// only update during market hours, which would make a localnet test flaky on
// nights/weekends. So for a runnable test we reuse the 24/7 SOL/USD feed as a
// STAND-IN for the share price. The mechanics are identical; only the feed id
// would change in production.
//
// Flow at runtime:
//   buy(shares):   read+validate price -> cost = shares*price
//                  USDC --transfer--> vault ; stock --mint(PDA)--> user
//   sell(shares):  read+validate price -> proceeds = shares*price
//                  stock --burn--> gone ; USDC --transfer(PDA)--> user
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, mint_to, transfer_checked, Burn, Mint, MintTo, TokenAccount, TokenInterface,
    TransferChecked,
};
use pyth_solana_receiver_sdk::price_update::{get_feed_id_from_hex, Price, PriceUpdateV2};

declare_id!("Stok6aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

// --- Oracle safety policy (same as P4/P5b) -----------------------------------
const MAX_PRICE_AGE_SECS: u64 = 120;
const MAX_CONFIDENCE_BPS: u128 = 100;
// SOL/USD, used here as a 24/7 stand-in for a stock price feed (see top note).
const SHARE_PRICE_FEED_ID: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

#[program]
pub mod p6a_tokenized_stock {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_market
    //
    // Records the stock + USDC mints and creates the USDC vault that holds sale
    // proceeds (and funds redemptions), owned by the `authority` PDA.
    // ------------------------------------------------------------------------
    pub fn initialize_market(ctx: Context<InitializeMarket>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        market.admin = ctx.accounts.admin.key();
        market.stock_mint = ctx.accounts.stock_mint.key();
        market.usdc_mint = ctx.accounts.usdc_mint.key();
        market.vault = ctx.accounts.vault.key();
        msg!("Stock market initialized (oracle-priced issuance)");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: buy — mint `shares` of the stock, paying oracle-priced USDC.
    // ------------------------------------------------------------------------
    pub fn buy(ctx: Context<Trade>, shares: u64) -> Result<()> {
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let cost = shares_to_usdc(
            shares,
            &price,
            ctx.accounts.usdc_mint.decimals,
            ctx.accounts.stock_mint.decimals,
        )?;
        require!(cost > 0, MarketError::AmountTooSmall);

        // (a) User pays USDC into the vault (user signs).
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.user_usdc_ata.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            cost,
            ctx.accounts.usdc_mint.decimals,
        )?;

        // (b) Program mints the stock shares to the user (PDA signs).
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.stock_mint.to_account_info(),
                    to: ctx.accounts.user_stock_ata.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer_seeds,
            ),
            shares,
        )?;

        msg!("Bought {} shares for {} USDC", shares, cost);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: sell — burn `shares` and pay out oracle-priced USDC.
    // ------------------------------------------------------------------------
    pub fn sell(ctx: Context<Trade>, shares: u64) -> Result<()> {
        let price = load_validated_price(&ctx.accounts.price_update)?;
        let proceeds = shares_to_usdc(
            shares,
            &price,
            ctx.accounts.usdc_mint.decimals,
            ctx.accounts.stock_mint.decimals,
        )?;
        require!(proceeds > 0, MarketError::AmountTooSmall);

        // (a) Burn the user's shares (user signs).
        burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.stock_mint.to_account_info(),
                    from: ctx.accounts.user_stock_ata.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            shares,
        )?;

        // (b) Pay USDC out of the vault (PDA signs).
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.user_usdc_ata.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer_seeds,
            ),
            proceeds,
            ctx.accounts.usdc_mint.decimals,
        )?;

        msg!("Sold {} shares for {} USDC", shares, proceeds);
        Ok(())
    }
}

// ============================================================================
// PRICING (same shape as P5b, one direction: shares -> USDC)
// ============================================================================

fn load_validated_price(price_update: &Account<PriceUpdateV2>) -> Result<Price> {
    let feed_id = get_feed_id_from_hex(SHARE_PRICE_FEED_ID)?;
    let price =
        price_update.get_price_no_older_than(&Clock::get()?, MAX_PRICE_AGE_SECS, &feed_id)?;
    require!(price.price > 0, MarketError::NonPositivePrice);
    let conf_bps = (price.conf as u128)
        .checked_mul(10_000)
        .and_then(|v| v.checked_div(price.price as u128))
        .ok_or(MarketError::MathOverflow)?;
    require!(conf_bps <= MAX_CONFIDENCE_BPS, MarketError::PriceTooUncertain);
    Ok(price)
}

// usdc = shares * price, adjusted for the two tokens' decimals:
//   usdc_raw = shares_raw * mantissa * 10^(exponent + usdc_dec - stock_dec)
fn shares_to_usdc(
    shares: u64,
    price: &Price,
    usdc_decimals: u8,
    stock_decimals: u8,
) -> Result<u64> {
    let shift = price.exponent + usdc_decimals as i32 - stock_decimals as i32;
    let base = (shares as u128)
        .checked_mul(price.price as u128)
        .ok_or(MarketError::MathOverflow)?;
    let scaled = if shift >= 0 {
        base.checked_mul(10u128.checked_pow(shift as u32).ok_or(MarketError::MathOverflow)?)
            .ok_or(MarketError::MathOverflow)?
    } else {
        base / 10u128.checked_pow((-shift) as u32).ok_or(MarketError::MathOverflow)?
    };
    u64::try_from(scaled).map_err(|_| MarketError::MathOverflow.into())
}

// ============================================================================
// STATE
// ============================================================================

#[account]
pub struct Market {
    pub admin: Pubkey,
    pub stock_mint: Pubkey,
    pub usdc_mint: Pubkey,
    pub vault: Pubkey,
}

impl Market {
    const SPACE: usize = 8 + 32 + 32 + 32 + 32;
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = Market::SPACE,
        seeds = [b"market"],
        bump
    )]
    pub market: Account<'info, Market>,

    // The tokenized-stock mint (created in the test; authority = PDA).
    pub stock_mint: InterfaceAccount<'info, Mint>,

    // The USDC mint (mock).
    pub usdc_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA that owns the vault and is the stock mint authority.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        seeds = [b"vault"],
        bump,
        token::mint = usdc_mint,
        token::authority = authority,
        token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

// buy and sell take the same accounts, so they share one context.
#[derive(Accounts)]
pub struct Trade<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"market"],
        bump,
        has_one = stock_mint,
        has_one = usdc_mint,
        has_one = vault,
    )]
    pub market: Account<'info, Market>,

    #[account(mut)]
    pub stock_mint: InterfaceAccount<'info, Mint>,

    pub usdc_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_stock_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_usdc_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

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
pub enum MarketError {
    #[msg("Price confidence band is too wide to trust")]
    PriceTooUncertain,
    #[msg("Price is zero or negative")]
    NonPositivePrice,
    #[msg("Amount too small — rounds to zero")]
    AmountTooSmall,
    #[msg("Arithmetic overflow while pricing")]
    MathOverflow,
}
