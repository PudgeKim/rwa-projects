// ============================================================================
// P2 test — proves the KYC gate end-to-end:
//   create mint (frozen-by-default) -> user ATA is frozen -> mint FAILS
//   -> admin verifies KYC (thaw) -> mint SUCCEEDS -> revoke (freeze) -> FAILS
//
// Read alongside lib.rs to see the client side of Token-2022 + the Config PDA.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P2KycToken } from "../target/types/p2_kyc_token";
import {
  TOKEN_2022_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  getAccount,
} from "@solana/spl-token";
import { PublicKey, Keypair } from "@solana/web3.js";
import { assert } from "chai";

describe("p2-kyc-token", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P2KycToken as Program<P2KycToken>;
  const payer = provider.wallet as anchor.Wallet; // also acts as admin
  const mint = Keypair.generate(); // the mint is a fresh keypair (raw-CPI created)
  const user = Keypair.generate(); // just a wallet address that will hold tokens

  // The Config PDA (mint + freeze authority, holder of the admin key).
  const [configPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("config")],
    program.programId
  );

  // The user's Associated Token Account for this Token-2022 mint.
  const userAta = getAssociatedTokenAddressSync(
    mint.publicKey,
    user.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID
  );

  const ONE_TOKEN = 1_000_000; // 6 decimals

  const isFrozen = async () =>
    (await getAccount(provider.connection, userAta, undefined, TOKEN_2022_PROGRAM_ID))
      .isFrozen;
  const balance = async () =>
    (await getAccount(provider.connection, userAta, undefined, TOKEN_2022_PROGRAM_ID))
      .amount;

  it("initializes config + a frozen-by-default mint", async () => {
    await program.methods
      .initialize()
      .accounts({
        payer: payer.publicKey,
        admin: payer.publicKey,
        mintAccount: mint.publicKey,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .signers([mint])
      .rpc();

    const config = await program.account.config.fetch(configPda);
    assert.ok(config.admin.equals(payer.publicKey));
  });

  it("creates a user account that is FROZEN by default", async () => {
    await program.methods
      .createUserTokenAccount()
      .accounts({
        payer: payer.publicKey,
        user: user.publicKey,
        mintAccount: mint.publicKey,
        userTokenAccount: userAta,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.isTrue(await isFrozen(), "new account should be frozen before KYC");
  });

  it("FAILS to mint to a frozen (un-KYC'd) account", async () => {
    let failed = false;
    try {
      await program.methods
        .mintToUser(new anchor.BN(100 * ONE_TOKEN))
        .accounts({
          admin: payer.publicKey,
          mintAccount: mint.publicKey,
          userTokenAccount: userAta,
          tokenProgram: TOKEN_2022_PROGRAM_ID,
        })
        .rpc();
    } catch {
      failed = true; // expected: token program rejects mint to a frozen account
    }
    assert.isTrue(failed, "minting to a frozen account must fail");
  });

  it("verifies KYC (thaws) and then mints successfully", async () => {
    await program.methods
      .verifyKyc()
      .accounts({
        admin: payer.publicKey,
        mintAccount: mint.publicKey,
        userTokenAccount: userAta,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.isFalse(await isFrozen(), "account should be thawed after KYC");

    await program.methods
      .mintToUser(new anchor.BN(100 * ONE_TOKEN))
      .accounts({
        admin: payer.publicKey,
        mintAccount: mint.publicKey,
        userTokenAccount: userAta,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal((await balance()).toString(), (100 * ONE_TOKEN).toString());
  });

  it("revokes KYC (re-freezes) and minting fails again", async () => {
    await program.methods
      .revokeKyc()
      .accounts({
        admin: payer.publicKey,
        mintAccount: mint.publicKey,
        userTokenAccount: userAta,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.isTrue(await isFrozen(), "account should be frozen again after revoke");

    let failed = false;
    try {
      await program.methods
        .mintToUser(new anchor.BN(1 * ONE_TOKEN))
        .accounts({
          admin: payer.publicKey,
          mintAccount: mint.publicKey,
          userTokenAccount: userAta,
          tokenProgram: TOKEN_2022_PROGRAM_ID,
        })
        .rpc();
    } catch {
      failed = true;
    }
    assert.isTrue(failed, "minting after revoke must fail");
  });
});
