// ============================================================================
// P2 — KYC-gated token (Token-2022 `Default Account State`)
//
// THE BIG IDEA: a regulated/RWA token where every new holder's account is
// FROZEN by default, and can only be used after an admin "verifies KYC" (thaws
// it). This is the foundation of permissioned tokens.
//
// New concepts vs P1:
//   1. Token-2022 (a SEPARATE program from the classic Token program) and its
//      "extensions" — here, `DefaultAccountState`.
//   2. Extensions are initialized with RAW CPIs, in a strict order:
//          create_account  ->  <init extension>  ->  initialize_mint2
//      (the extension must be set BEFORE the mint is initialized).
//   3. A CUSTOM program data account (`Config`) that stores the admin and acts
//      as the mint/freeze authority — P1 had no custom state.
//   4. freeze / thaw CPIs, authorized by a PDA (reuses P1's `with_signer`).
// ============================================================================

use anchor_lang::prelude::*;
use anchor_lang::system_program::{create_account, CreateAccount};
use anchor_spl::{
    associated_token::AssociatedToken,
    token_2022::{
        initialize_mint2,
        spl_token_2022::{extension::ExtensionType, pod::PodMint, state::AccountState},
        InitializeMint2,
    },
    token_interface::{
        default_account_state_initialize, freeze_account, mint_to, thaw_account,
        DefaultAccountStateInitialize, FreezeAccount, Mint, MintTo, ThawAccount, TokenAccount,
        Token2022,
    },
};

declare_id!("3pX5NKLru1UBDVckynWQxsgnJeUN3N1viy36Gk9TSn8d");

#[program]
pub mod p2_kyc_token {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize
    //
    // Creates (a) the program's Config account (storing the admin), and
    //         (b) a Token-2022 mint whose new accounts are FROZEN by default.
    //
    // Notice the mint is NOT created by Anchor's `init` constraint — extensions
    // require manual setup, so we do three raw CPIs in order.
    // ------------------------------------------------------------------------
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        // Save who is allowed to verify KYC, and the config PDA bump (so later
        // instructions can sign as this PDA).
        ctx.accounts.config.admin = ctx.accounts.admin.key();
        ctx.accounts.config.bump = ctx.bumps.config;

        // --- Step 1: compute the mint size INCLUDING the extension ---
        // A plain mint is 82 bytes; the DefaultAccountState extension needs more.
        let mint_size =
            ExtensionType::try_calculate_account_len::<PodMint>(&[ExtensionType::DefaultAccountState])?;
        let lamports = Rent::get()?.minimum_balance(mint_size);

        // --- Step 2: allocate the mint account, owned by the Token-2022 program ---
        // The mint is a fresh keypair (a Signer), so this is a plain `invoke`:
        // the new account authorizes its own creation with its own signature.
        create_account(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                CreateAccount {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.mint_account.to_account_info(),
                },
            ),
            lamports,
            mint_size as u64,
            &ctx.accounts.token_program.key(), // owner = Token-2022 program
        )?;

        // --- Step 3: initialize the DefaultAccountState extension (BEFORE the mint) ---
        // This is what makes every future token account start FROZEN.
        default_account_state_initialize(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                DefaultAccountStateInitialize {
                    token_program_id: ctx.accounts.token_program.to_account_info(),
                    mint: ctx.accounts.mint_account.to_account_info(),
                },
            ),
            &AccountState::Frozen,
        )?;

        // --- Step 4: initialize the mint itself ---
        // mint authority AND freeze authority = the Config PDA, so the PROGRAM
        // controls minting and freezing/thawing.
        let config_key = ctx.accounts.config.key();
        initialize_mint2(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                InitializeMint2 {
                    mint: ctx.accounts.mint_account.to_account_info(),
                },
            ),
            6,                  // decimals
            &config_key,        // mint authority
            Some(&config_key),  // freeze authority
        )?;

        msg!("Mint created: new token accounts will be FROZEN until KYC");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: create_user_token_account
    //
    // Creates a user's ATA. Because of the extension, it is born FROZEN — the
    // user cannot receive or move tokens yet. (Body is empty; Anchor's
    // associated_token init does the work.)
    // ------------------------------------------------------------------------
    pub fn create_user_token_account(_ctx: Context<CreateUserTokenAccount>) -> Result<()> {
        msg!("User token account created (frozen by default)");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: verify_kyc  (THAW)
    //
    // The admin approves a user — the program thaws their account so it becomes
    // usable. Authorized by the Config PDA via `with_signer` (just like P1's
    // mint authority).
    // ------------------------------------------------------------------------
    pub fn verify_kyc(ctx: Context<UpdateUserState>) -> Result<()> {
        let signer_seeds: &[&[&[u8]]] = &[&[b"config", &[ctx.accounts.config.bump]]];
        thaw_account(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                ThawAccount {
                    account: ctx.accounts.user_token_account.to_account_info(),
                    mint: ctx.accounts.mint_account.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
            )
            .with_signer(signer_seeds),
        )?;
        msg!("KYC verified: account thawed");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 4: revoke_kyc  (FREEZE)
    //
    // The admin revokes a user — re-freeze their account. Symmetric to thaw.
    // ------------------------------------------------------------------------
    pub fn revoke_kyc(ctx: Context<UpdateUserState>) -> Result<()> {
        let signer_seeds: &[&[&[u8]]] = &[&[b"config", &[ctx.accounts.config.bump]]];
        freeze_account(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                FreezeAccount {
                    account: ctx.accounts.user_token_account.to_account_info(),
                    mint: ctx.accounts.mint_account.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
            )
            .with_signer(signer_seeds),
        )?;
        msg!("KYC revoked: account frozen");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 5: mint_to_user
    //
    // Mint tokens to a user. This FAILS if the account is still frozen (no KYC)
    // — that failure is the whole point of the gate. Authority = Config PDA.
    // ------------------------------------------------------------------------
    pub fn mint_to_user(ctx: Context<MintToUser>, amount: u64) -> Result<()> {
        let signer_seeds: &[&[&[u8]]] = &[&[b"config", &[ctx.accounts.config.bump]]];
        mint_to(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.mint_account.to_account_info(),
                    to: ctx.accounts.user_token_account.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
            )
            .with_signer(signer_seeds),
            amount,
        )?;
        Ok(())
    }
}

// ============================================================================
// CUSTOM STATE
// ============================================================================

// Our first program-owned data account. `InitSpace` auto-computes the byte size
// of the fields; the leading 8 bytes are Anchor's account discriminator.
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey, // who may verify/revoke KYC
    pub bump: u8,      // the config PDA bump, stored so we can sign later
}

// ============================================================================
// ACCOUNTS STRUCTS
// ============================================================================

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    // The future admin (stored into Config). A separate signer from payer so
    // you can see the distinction; in tests they can be the same wallet.
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = payer,
        space = 8 + Config::INIT_SPACE,   // 8 = discriminator
        seeds = [b"config"],
        bump
    )]
    pub config: Account<'info, Config>,

    // The mint is a fresh keypair we create by hand (raw CPI), so it's a Signer,
    // not an `init` mint. It must be `mut` because we write its data.
    #[account(mut)]
    pub mint_account: Signer<'info>,

    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CreateUserTokenAccount<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: only used as the owner/authority of the new ATA.
    pub user: UncheckedAccount<'info>,

    #[account(mut)]
    pub mint_account: InterfaceAccount<'info, Mint>,

    // The user's ATA. Created here, FROZEN by default thanks to the extension.
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = mint_account,
        associated_token::authority = user,
        associated_token::token_program = token_program,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// Shared by verify_kyc (thaw) and revoke_kyc (freeze).
#[derive(Accounts)]
pub struct UpdateUserState<'info> {
    pub admin: Signer<'info>,

    // `has_one = admin` enforces: config.admin == admin.key().
    // This is the authorization check — only the stored admin may proceed.
    #[account(
        seeds = [b"config"],
        bump = config.bump,
        has_one = admin
    )]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub mint_account: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct MintToUser<'info> {
    pub admin: Signer<'info>,

    #[account(
        seeds = [b"config"],
        bump = config.bump,
        has_one = admin
    )]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub mint_account: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
}
