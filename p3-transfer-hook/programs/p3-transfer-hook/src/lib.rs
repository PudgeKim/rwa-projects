// ============================================================================
// P3a — Compliance Transfer Hook (allowlist on every transfer)
//
// THE BIG IDEA: a Token-2022 mint can name a SEPARATE program — a "transfer
// hook" — that Token-2022 will CALL on EVERY transfer. If that program returns
// an error, the transfer fails. This lets us enforce arbitrary rules per
// transfer. Here the rule is an ALLOWLIST: the receiver's wallet must be on a
// white-list, or the transfer is rejected.
//
// How P3 differs from P2 (this is the key mental shift):
//   - P2: our program OWNED the token (it was the mint/freeze authority) and we
//     froze/thawed whole accounts. Coarse: an account is usable or not.
//   - P3: our program does NOT own the mint. It is a reusable RULE-CHECKER that
//     Token-2022 calls into. Many different mints could point at this one hook.
//     Fine-grained: the check runs on every single transfer.
//
// New concepts vs P2:
//   1. The "transfer hook interface" — a standard instruction (`Execute`) that
//      Token-2022 invokes via CPI on each transfer. We implement it with
//      `#[interface(spl_transfer_hook_interface::execute)]`.
//   2. The `ExtraAccountMetaList` — a PDA that tells Token-2022 WHICH EXTRA
//      accounts to pass to our hook (here: the white-list account). Token-2022
//      reads this list and auto-resolves those accounts for every transfer.
//   3. The hook is called with the token accounts in a "transferring" state.
//
// Flow at runtime:
//   user calls transfer_checked on Token-2022
//      -> Token-2022 moves the tokens
//      -> Token-2022 reads our ExtraAccountMetaList PDA
//      -> Token-2022 CPIs into OUR `transfer_hook` (Execute) with the resolved
//         extra accounts (the white-list)
//      -> we check the receiver is allowed; Err => the WHOLE transfer reverts
// ============================================================================

use anchor_lang::prelude::*;
use anchor_lang::system_program::{create_account, CreateAccount};
use anchor_spl::token_interface::{Mint, TokenAccount};

// These two crates are the "transfer hook" standard library. They are external
// to anchor-spl and are the version-sensitive part of this project.
//   - spl_tlv_account_resolution: builds/stores the ExtraAccountMetaList.
//   - spl_transfer_hook_interface: defines the `Execute` instruction shape.
use spl_tlv_account_resolution::{
    account::ExtraAccountMeta, seeds::Seed, state::ExtraAccountMetaList,
};
use spl_transfer_hook_interface::instruction::ExecuteInstruction;

declare_id!("HookP3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

// Max wallets we pre-allocate room for in the white-list (keeps the account a
// fixed size, so we never need to `realloc`). Bump this if you want more.
const MAX_WALLETS: usize = 10;

#[program]
pub mod p3_transfer_hook {
    use super::*;

    // ------------------------------------------------------------------------
    // Instruction 1: initialize_white_list
    //
    // Creates the single, global white-list account (a normal program-owned
    // data account, like P2's Config). Stores who may edit it (`authority`) and
    // an initially-empty list of allowed wallets.
    // ------------------------------------------------------------------------
    pub fn initialize_white_list(ctx: Context<InitializeWhiteList>) -> Result<()> {
        ctx.accounts.white_list.authority = ctx.accounts.authority.key();
        ctx.accounts.white_list.wallets = Vec::new();
        msg!("White-list initialized (empty)");
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 2: add_to_white_list
    //
    // The authority adds a wallet that is allowed to RECEIVE the token. Only the
    // stored authority may call this (`has_one = authority`).
    // ------------------------------------------------------------------------
    pub fn add_to_white_list(ctx: Context<UpdateWhiteList>, wallet: Pubkey) -> Result<()> {
        let list = &mut ctx.accounts.white_list;
        require!(list.wallets.len() < MAX_WALLETS, HookError::WhiteListFull);
        if !list.wallets.contains(&wallet) {
            list.wallets.push(wallet);
        }
        msg!("Added to white-list: {}", wallet);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 3: initialize_extra_account_meta_list
    //
    // Per MINT. Creates the `ExtraAccountMetaList` PDA that Token-2022 reads to
    // learn which EXTRA accounts to hand our hook on each transfer. Here there
    // is exactly one extra account: the white-list PDA.
    //
    // We create it with a RAW `create_account` CPI (just like P2's mint) because
    // its size is dynamic — it depends on how many extra metas we store — so
    // Anchor's `init` can't size it for us.
    // ------------------------------------------------------------------------
    pub fn initialize_extra_account_meta_list(
        ctx: Context<InitializeExtraAccountMetaList>,
    ) -> Result<()> {
        // Describe the extra accounts the hook needs. ExtraAccountMeta can point
        // to a literal pubkey, an account passed at a known index, or — as here —
        // a PDA derived from seeds. Seed::Literal[b"white_list"] resolves to our
        // global white-list PDA (derived from THIS program's id).
        let account_metas = vec![ExtraAccountMeta::new_with_seeds(
            &[Seed::Literal {
                bytes: b"white_list".to_vec(),
            }],
            false, // is_signer  — the hook only reads it
            false, // is_writable — read-only
        )?];

        // Compute the exact byte size for this many metas, then allocate+fund the
        // PDA, owned by THIS program (so only we can write the list).
        let account_size = ExtraAccountMetaList::size_of(account_metas.len())? as u64;
        let lamports = Rent::get()?.minimum_balance(account_size as usize);

        let mint = ctx.accounts.mint.key();
        // The PDA seeds MUST be exactly [b"extra-account-metas", mint] — this is
        // the fixed address Token-2022 looks up for a given mint. (It's part of
        // the transfer-hook interface convention.)
        let signer_seeds: &[&[&[u8]]] =
            &[&[b"extra-account-metas", mint.as_ref(), &[ctx.bumps.extra_account_meta_list]]];

        create_account(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                CreateAccount {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.extra_account_meta_list.to_account_info(),
                },
            )
            .with_signer(signer_seeds), // the PDA "signs" its own creation (it has no key)
            lamports,
            account_size,
            ctx.program_id, // owner = this hook program
        )?;

        // Write the TLV-encoded meta list into the freshly-allocated account.
        // `ExecuteInstruction` ties these metas to the Execute (transfer) call.
        let mut data = ctx.accounts.extra_account_meta_list.try_borrow_mut_data()?;
        ExtraAccountMetaList::init::<ExecuteInstruction>(&mut data, &account_metas)?;

        msg!("ExtraAccountMetaList ready for mint {}", mint);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Instruction 4: transfer_hook  (the Execute interface)
    //
    // Token-2022 CPIs into THIS on every transfer of the mint. The `#[interface]`
    // attribute maps it to the standard `Execute` discriminator so Token-2022
    // can find it. We receive the source, mint, destination, owner (fixed by the
    // interface) PLUS our resolved extra account (the white-list).
    //
    // THE RULE: the destination (receiver) wallet must be on the white-list, or
    // we return an error — which makes Token-2022 revert the entire transfer.
    // ------------------------------------------------------------------------
    #[interface(spl_transfer_hook_interface::execute)]
    pub fn transfer_hook(ctx: Context<TransferHook>, amount: u64) -> Result<()> {
        // The receiver's wallet = the OWNER of the destination token account.
        let receiver = ctx.accounts.destination_token.owner;

        require!(
            ctx.accounts.white_list.wallets.contains(&receiver),
            HookError::ReceiverNotWhiteListed
        );

        msg!("Transfer of {} allowed: receiver {} is white-listed", amount, receiver);
        Ok(())
    }
}

// ============================================================================
// STATE
// ============================================================================

// The global allow-list. Pre-sized for MAX_WALLETS so it never needs realloc.
#[account]
pub struct WhiteList {
    pub authority: Pubkey,    // who may add wallets
    pub wallets: Vec<Pubkey>, // allowed receivers
}

impl WhiteList {
    // 8 discriminator + 32 authority + 4 Vec length prefix + 32 * capacity
    const SPACE: usize = 8 + 32 + 4 + (32 * MAX_WALLETS);
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct InitializeWhiteList<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = WhiteList::SPACE,
        seeds = [b"white_list"],
        bump
    )]
    pub white_list: Account<'info, WhiteList>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateWhiteList<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [b"white_list"],
        bump,
        has_one = authority // only the stored authority may edit the list
    )]
    pub white_list: Account<'info, WhiteList>,
}

#[derive(Accounts)]
pub struct InitializeExtraAccountMetaList<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: PDA created and written by hand below (raw create_account + TLV init).
    /// Address is fixed by the interface: ["extra-account-metas", mint].
    #[account(
        mut,
        seeds = [b"extra-account-metas", mint.key().as_ref()],
        bump
    )]
    pub extra_account_meta_list: AccountInfo<'info>,

    // The mint this meta-list belongs to. (We only read its key.)
    pub mint: InterfaceAccount<'info, Mint>,

    pub system_program: Program<'info, System>,
}

// The account set for the Execute (transfer_hook) call. The FIRST FOUR accounts
// are fixed by the transfer-hook interface and MUST be in this exact order:
//   0: source token account, 1: mint, 2: destination token account, 3: owner.
// After those come our EXTRA accounts, in the same order as the meta-list:
//   4: extra_account_meta_list, 5: white_list.
#[derive(Accounts)]
pub struct TransferHook<'info> {
    #[account(token::mint = mint, token::token_program = anchor_spl::token_2022::ID)]
    pub source_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(token::mint = mint, token::token_program = anchor_spl::token_2022::ID)]
    pub destination_token: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: the source token account's owner; not read here, fixed by interface.
    pub owner: UncheckedAccount<'info>,

    /// CHECK: validated by its PDA seeds; Token-2022 passes the same PDA we created.
    #[account(
        seeds = [b"extra-account-metas", mint.key().as_ref()],
        bump
    )]
    pub extra_account_meta_list: UncheckedAccount<'info>,

    // Our resolved extra account: the white-list we check against.
    #[account(seeds = [b"white_list"], bump)]
    pub white_list: Account<'info, WhiteList>,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum HookError {
    #[msg("Receiver wallet is not on the white-list")]
    ReceiverNotWhiteListed,
    #[msg("White-list is full")]
    WhiteListFull,
}
