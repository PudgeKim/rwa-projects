// ============================================================================
// P6b — Mini Constant-Product AMM (secondary market for the tokenized stock)
//
// THE BIG IDEA: P6a was the PRIMARY market — the issuer mints/redeems the stock
// at the oracle price. P6b is the SECONDARY market — an Automated Market Maker
// where the token trades freely and its price emerges from a POOL of reserves,
// no oracle and no order book. This is how xStocks tokens actually trade on
// Orca / Raydium / Jupiter.
//
// The whole thing rests on ONE invariant — constant product:
//
//     reserve_a * reserve_b = k   (must not decrease on a swap)
//
// A pool holds reserves of two tokens (here: tokenized stock "A" and USDC "B").
//   - The PRICE is just the ratio reserve_b / reserve_a. Buying A removes A and
//     adds B, so A gets scarcer and pricier — the pool self-adjusts.
//   - LIQUIDITY PROVIDERS deposit both tokens in ratio and receive LP tokens
//     representing their share; they earn the swap fee.
//   - A SWAP puts `amount_in` of one side in and takes `amount_out` of the other
//     out, keeping the product ~constant. A fee (e.g. 0.3%) is skimmed from the
//     input, which is what pays the LPs.
//
// How the AMM price relates to P6a's oracle price: if they diverge, arbitrageurs
// buy on the cheap side and sell on the dear side until they meet. That
// arbitrage is what keeps a tokenized stock's market price pinned to the real
// share price — the key insight of the whole P6 project.
//
// New concepts vs P1–P6a:
//   - A pool with TWO reserve vaults owned by one PDA.
//   - An LP mint whose supply tracks total liquidity (mint on deposit, burn on
//     withdraw); first deposit uses sqrt(a*b) to set the initial LP supply.
//   - The x*y=k swap formula with a fee, and slippage protection (min_out).
// ============================================================================

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, mint_to, transfer_checked, Burn, Mint, MintTo, TokenAccount, TokenInterface,
    TransferChecked,
};

declare_id!("Amm6baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

#[program]
pub mod p6b_mini_amm {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_pool
    //
    // Creates the pool config and the two reserve vaults (owned by the pool
    // `authority` PDA). The LP mint is created client-side with its authority
    // set to the same PDA, so only this program can mint/burn LP tokens.
    // ------------------------------------------------------------------------
    pub fn initialize_pool(ctx: Context<InitializePool>, fee_bps: u16) -> Result<()> {
        require!(fee_bps < 10_000, AmmError::BadFee);
        let pool = &mut ctx.accounts.pool;
        pool.token_a_mint = ctx.accounts.token_a_mint.key();
        pool.token_b_mint = ctx.accounts.token_b_mint.key();
        pool.lp_mint = ctx.accounts.lp_mint.key();
        pool.reserve_a = ctx.accounts.reserve_a.key();
        pool.reserve_b = ctx.accounts.reserve_b.key();
        pool.fee_bps = fee_bps;
        msg!("Pool initialized (fee {} bps)", fee_bps);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: add_liquidity
    //
    // Deposit both tokens and receive LP tokens. The FIRST deposit sets the
    // price (any ratio) and mints sqrt(a*b) LP. Later deposits must match the
    // current reserve ratio; we anchor on `amount_a` and derive the required B.
    // ------------------------------------------------------------------------
    pub fn add_liquidity(
        ctx: Context<AddLiquidity>,
        amount_a: u64,
        max_b: u64,
        min_lp: u64,
    ) -> Result<()> {
        let reserve_a = ctx.accounts.reserve_a.amount as u128;
        let reserve_b = ctx.accounts.reserve_b.amount as u128;
        let lp_supply = ctx.accounts.lp_mint.supply as u128;

        let (deposit_a, deposit_b, lp_out) = if lp_supply == 0 {
            // First provider: take amounts as given, LP = sqrt(a*b).
            let lp = isqrt((amount_a as u128).checked_mul(max_b as u128).ok_or(AmmError::Overflow)?);
            (amount_a as u128, max_b as u128, lp)
        } else {
            // Match the current ratio: required_b = amount_a * reserve_b / reserve_a.
            let required_b = (amount_a as u128)
                .checked_mul(reserve_b)
                .ok_or(AmmError::Overflow)?
                / reserve_a;
            require!(required_b <= max_b as u128, AmmError::SlippageExceeded);
            // LP minted in proportion to the pool you're adding.
            let lp = (amount_a as u128)
                .checked_mul(lp_supply)
                .ok_or(AmmError::Overflow)?
                / reserve_a;
            (amount_a as u128, required_b, lp)
        };

        require!(lp_out > 0, AmmError::ZeroLiquidity);
        require!(lp_out >= min_lp as u128, AmmError::SlippageExceeded);

        // Pull both tokens from the user into the reserves (user signs).
        transfer_from_user(
            &ctx.accounts.token_program,
            &ctx.accounts.user_a,
            &ctx.accounts.token_a_mint,
            &ctx.accounts.reserve_a,
            &ctx.accounts.user,
            deposit_a as u64,
            ctx.accounts.token_a_mint.decimals,
        )?;
        transfer_from_user(
            &ctx.accounts.token_program,
            &ctx.accounts.user_b,
            &ctx.accounts.token_b_mint,
            &ctx.accounts.reserve_b,
            &ctx.accounts.user,
            deposit_b as u64,
            ctx.accounts.token_b_mint.decimals,
        )?;

        // Mint LP tokens to the user (PDA signs).
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    to: ctx.accounts.user_lp.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer_seeds,
            ),
            lp_out as u64,
        )?;

        msg!("Added liquidity: {} A + {} B -> {} LP", deposit_a, deposit_b, lp_out);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: remove_liquidity
    //
    // Burn LP tokens and receive a proportional slice of BOTH reserves.
    // ------------------------------------------------------------------------
    pub fn remove_liquidity(
        ctx: Context<RemoveLiquidity>,
        lp_amount: u64,
        min_a: u64,
        min_b: u64,
    ) -> Result<()> {
        let reserve_a = ctx.accounts.reserve_a.amount as u128;
        let reserve_b = ctx.accounts.reserve_b.amount as u128;
        let lp_supply = ctx.accounts.lp_mint.supply as u128;
        require!(lp_supply > 0, AmmError::ZeroLiquidity);

        // Your share of each reserve = lp_amount / lp_supply.
        let out_a = (lp_amount as u128).checked_mul(reserve_a).ok_or(AmmError::Overflow)? / lp_supply;
        let out_b = (lp_amount as u128).checked_mul(reserve_b).ok_or(AmmError::Overflow)? / lp_supply;
        require!(out_a >= min_a as u128 && out_b >= min_b as u128, AmmError::SlippageExceeded);

        // Burn the LP tokens (user signs).
        burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    from: ctx.accounts.user_lp.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            lp_amount,
        )?;

        // Pay out both tokens from the reserves (PDA signs).
        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];
        transfer_from_reserve(
            &ctx.accounts.token_program, &ctx.accounts.reserve_a, &ctx.accounts.token_a_mint,
            &ctx.accounts.user_a, &ctx.accounts.authority, out_a as u64,
            ctx.accounts.token_a_mint.decimals, signer_seeds,
        )?;
        transfer_from_reserve(
            &ctx.accounts.token_program, &ctx.accounts.reserve_b, &ctx.accounts.token_b_mint,
            &ctx.accounts.user_b, &ctx.accounts.authority, out_b as u64,
            ctx.accounts.token_b_mint.decimals, signer_seeds,
        )?;

        msg!("Removed liquidity: {} LP -> {} A + {} B", lp_amount, out_a, out_b);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 4: swap
    //
    // Trade `amount_in` of one side for the other, keeping reserve_a*reserve_b
    // roughly constant. `a_to_b = true` means pay token A, receive token B.
    // `min_out` protects the trader from slippage / front-running.
    // ------------------------------------------------------------------------
    pub fn swap(ctx: Context<Swap>, amount_in: u64, min_out: u64, a_to_b: bool) -> Result<()> {
        require!(amount_in > 0, AmmError::ZeroLiquidity);
        let reserve_a = ctx.accounts.reserve_a.amount;
        let reserve_b = ctx.accounts.reserve_b.amount;

        // Pick in/out reserves by direction.
        let (reserve_in, reserve_out) = if a_to_b {
            (reserve_a, reserve_b)
        } else {
            (reserve_b, reserve_a)
        };
        let amount_out = get_amount_out(amount_in, reserve_in, reserve_out, ctx.accounts.pool.fee_bps)?;
        require!(amount_out >= min_out, AmmError::SlippageExceeded);

        let bump = ctx.bumps.authority;
        let signer_seeds: &[&[&[u8]]] = &[&[b"authority", &[bump]]];

        if a_to_b {
            // A in (user signs), B out (PDA signs).
            transfer_from_user(
                &ctx.accounts.token_program, &ctx.accounts.user_a, &ctx.accounts.token_a_mint,
                &ctx.accounts.reserve_a, &ctx.accounts.user, amount_in, ctx.accounts.token_a_mint.decimals,
            )?;
            transfer_from_reserve(
                &ctx.accounts.token_program, &ctx.accounts.reserve_b, &ctx.accounts.token_b_mint,
                &ctx.accounts.user_b, &ctx.accounts.authority, amount_out, ctx.accounts.token_b_mint.decimals, signer_seeds,
            )?;
        } else {
            // B in (user signs), A out (PDA signs).
            transfer_from_user(
                &ctx.accounts.token_program, &ctx.accounts.user_b, &ctx.accounts.token_b_mint,
                &ctx.accounts.reserve_b, &ctx.accounts.user, amount_in, ctx.accounts.token_b_mint.decimals,
            )?;
            transfer_from_reserve(
                &ctx.accounts.token_program, &ctx.accounts.reserve_a, &ctx.accounts.token_a_mint,
                &ctx.accounts.user_a, &ctx.accounts.authority, amount_out, ctx.accounts.token_a_mint.decimals, signer_seeds,
            )?;
        }

        msg!("Swapped {} in -> {} out (a_to_b={})", amount_in, amount_out, a_to_b);
        Ok(())
    }
}

// ============================================================================
// AMM MATH
// ============================================================================

// Constant-product output with a fee taken from the input (Uniswap v2 style):
//   in_after_fee = amount_in * (1 - fee)
//   amount_out   = reserve_out * in_after_fee / (reserve_in + in_after_fee)
fn get_amount_out(amount_in: u64, reserve_in: u64, reserve_out: u64, fee_bps: u16) -> Result<u64> {
    require!(reserve_in > 0 && reserve_out > 0, AmmError::ZeroLiquidity);
    let in_after_fee = (amount_in as u128)
        .checked_mul((10_000 - fee_bps) as u128)
        .ok_or(AmmError::Overflow)?;
    let numerator = in_after_fee.checked_mul(reserve_out as u128).ok_or(AmmError::Overflow)?;
    let denominator = (reserve_in as u128)
        .checked_mul(10_000)
        .ok_or(AmmError::Overflow)?
        .checked_add(in_after_fee)
        .ok_or(AmmError::Overflow)?;
    u64::try_from(numerator / denominator).map_err(|_| AmmError::Overflow.into())
}

// Integer square root (Newton's method) for the first LP mint = sqrt(a*b).
fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ---- small CPI helpers so the instructions above stay readable --------------

fn transfer_from_user<'info>(
    token_program: &Interface<'info, TokenInterface>,
    from: &InterfaceAccount<'info, TokenAccount>,
    mint: &InterfaceAccount<'info, Mint>,
    to: &InterfaceAccount<'info, TokenAccount>,
    authority: &Signer<'info>,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    transfer_checked(
        CpiContext::new(
            token_program.to_account_info(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to: to.to_account_info(),
                authority: authority.to_account_info(),
            },
        ),
        amount,
        decimals,
    )
}

fn transfer_from_reserve<'info>(
    token_program: &Interface<'info, TokenInterface>,
    from: &InterfaceAccount<'info, TokenAccount>,
    mint: &InterfaceAccount<'info, Mint>,
    to: &InterfaceAccount<'info, TokenAccount>,
    authority: &UncheckedAccount<'info>,
    amount: u64,
    decimals: u8,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to: to.to_account_info(),
                authority: authority.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        decimals,
    )
}

// ============================================================================
// STATE
// ============================================================================

#[account]
pub struct Pool {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub reserve_a: Pubkey,
    pub reserve_b: Pubkey,
    pub fee_bps: u16,
}

impl Pool {
    const SPACE: usize = 8 + 32 * 5 + 2;
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(init, payer = admin, space = Pool::SPACE, seeds = [b"pool"], bump)]
    pub pool: Account<'info, Pool>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,

    // LP mint, created client-side with authority = the pool authority PDA.
    pub lp_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA that owns both reserves and is the LP mint authority.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(
        init, payer = admin, seeds = [b"reserve_a"], bump,
        token::mint = token_a_mint, token::authority = authority, token::token_program = token_program,
    )]
    pub reserve_a: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init, payer = admin, seeds = [b"reserve_b"], bump,
        token::mint = token_b_mint, token::authority = authority, token::token_program = token_program,
    )]
    pub reserve_b: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

// add_liquidity / remove_liquidity share the same account set.
#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"pool"], bump,
        has_one = token_a_mint, has_one = token_b_mint,
        has_one = lp_mint, has_one = reserve_a, has_one = reserve_b,
    )]
    pub pool: Account<'info, Pool>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub lp_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub reserve_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub reserve_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_b: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_lp: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: reserve-owner / LP-mint-authority PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct RemoveLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"pool"], bump,
        has_one = token_a_mint, has_one = token_b_mint,
        has_one = lp_mint, has_one = reserve_a, has_one = reserve_b,
    )]
    pub pool: Account<'info, Pool>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub lp_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub reserve_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub reserve_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_b: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_lp: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: reserve-owner / LP-mint-authority PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"pool"], bump,
        has_one = token_a_mint, has_one = token_b_mint,
        has_one = reserve_a, has_one = reserve_b,
    )]
    pub pool: Account<'info, Pool>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub reserve_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub reserve_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_b: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: reserve-owner PDA.
    #[account(seeds = [b"authority"], bump)]
    pub authority: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum AmmError {
    #[msg("Fee must be < 100%")]
    BadFee,
    #[msg("Pool has no liquidity")]
    ZeroLiquidity,
    #[msg("Slippage exceeded / output below minimum")]
    SlippageExceeded,
    #[msg("Arithmetic overflow")]
    Overflow,
}
