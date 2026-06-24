// ============================================================================
// P1 test — drives the three instructions end-to-end against a local validator.
//
// `anchor test` will: build the program, start a local validator, deploy, then
// run this file. Read it top-to-bottom alongside lib.rs to see how a CLIENT
// talks to the program.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P1SplToken } from "../target/types/p1_spl_token";
import {
  getAssociatedTokenAddressSync,
  getAccount,
} from "@solana/spl-token";
import { PublicKey, Keypair } from "@solana/web3.js";
import { assert } from "chai";

describe("p1-spl-token", () => {
  // The provider reads your local Solana wallet + cluster from the environment.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P1SplToken as Program<P1SplToken>;
  const payer = provider.wallet as anchor.Wallet;

  // Our mint is a PDA at seeds = ["mint"]. We derive the SAME address the
  // program derives, so client and program agree on which account is the mint.
  const [mintPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("mint")],
    program.programId
  );

  // 6 decimals -> 1 token = 1_000_000 base units.
  const ONE_TOKEN = 1_000_000;

  it("initializes the mint", async () => {
    await program.methods
      .initializeMint()
      .accounts({ payer: payer.publicKey })
      .rpc();

    // The mint PDA should now exist on-chain.
    const mintInfo = await provider.connection.getAccountInfo(mintPda);
    assert.isNotNull(mintInfo, "mint account should exist");
  });

  it("mints 1000 tokens to the payer", async () => {
    const payerAta = getAssociatedTokenAddressSync(mintPda, payer.publicKey);

    await program.methods
      .mintTokens(new anchor.BN(1000 * ONE_TOKEN))
      .accounts({
        payer: payer.publicKey,
        recipient: payer.publicKey,
      })
      .rpc();

    const acct = await getAccount(provider.connection, payerAta);
    assert.equal(acct.amount.toString(), (1000 * ONE_TOKEN).toString());
  });

  it("transfers 250 tokens to a new recipient", async () => {
    const recipient = Keypair.generate().publicKey;
    const payerAta = getAssociatedTokenAddressSync(mintPda, payer.publicKey);
    const recipientAta = getAssociatedTokenAddressSync(mintPda, recipient);

    await program.methods
      .transferTokens(new anchor.BN(250 * ONE_TOKEN))
      .accounts({
        sender: payer.publicKey,
        mint: mintPda,
        recipient: recipient,
      })
      .rpc();

    const fromAcct = await getAccount(provider.connection, payerAta);
    const toAcct = await getAccount(provider.connection, recipientAta);

    assert.equal(fromAcct.amount.toString(), (750 * ONE_TOKEN).toString());
    assert.equal(toAcct.amount.toString(), (250 * ONE_TOKEN).toString());
  });
});
