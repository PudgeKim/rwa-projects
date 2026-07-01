// ============================================================================
// P3b test — proves issuer clawback end-to-end:
//   create a Token-2022 mint whose PERMANENT DELEGATE = our program's PDA
//   -> init config (admin)
//   -> mint 100 tokens to a "victim" wallet
//   -> admin calls clawback => tokens move victim -> recovery, victim NEVER signs
//   -> admin calls burn_from => remaining tokens destroyed
//
// The whole point to watch: the victim's keypair is NOT in the signer list of
// the clawback / burn transactions. Only the admin signs. That is the power the
// permanent-delegate extension grants — and our program gates it behind admin.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P3bPermanentDelegate } from "../target/types/p3b_permanent_delegate";
import {
  TOKEN_2022_PROGRAM_ID,
  ExtensionType,
  getMintLen,
  createInitializeMintInstruction,
  createInitializePermanentDelegateInstruction,
  createAssociatedTokenAccountInstruction,
  getAssociatedTokenAddressSync,
  mintTo,
  getAccount,
  getMint,
} from "@solana/spl-token";
import {
  PublicKey,
  Keypair,
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { assert } from "chai";

describe("p3b-permanent-delegate", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace
    .P3bPermanentDelegate as Program<P3bPermanentDelegate>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet; // payer + mint authority + admin
  const payer = wallet.payer;

  const mint = Keypair.generate();
  const victim = Keypair.generate(); // an ordinary holder we will claw back from
  const DECIMALS = 6;
  const ONE = 10 ** DECIMALS;

  // PDAs owned by our program.
  const [configPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("config")],
    program.programId
  );
  // This PDA is the mint's PERMANENT DELEGATE. It's just an address; only our
  // program can sign as it, so only our (admin-gated) instructions can use it.
  const [delegatePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("delegate")],
    program.programId
  );

  const victimAta = getAssociatedTokenAddressSync(
    mint.publicKey,
    victim.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID
  );
  const recoveryAta = getAssociatedTokenAddressSync(
    mint.publicKey,
    payer.publicKey, // issuer's recovery account
    false,
    TOKEN_2022_PROGRAM_ID
  );

  const balance = async (ata: PublicKey) =>
    (await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount;

  it("creates a Token-2022 mint whose permanent delegate is our PDA", async () => {
    // Order matters (same rule as P2/P3a): allocate -> init extension -> init mint.
    const mintLen = getMintLen([ExtensionType.PermanentDelegate]);
    const lamports = await connection.getMinimumBalanceForRentExemption(mintLen);

    const tx = new Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: mint.publicKey,
        space: mintLen,
        lamports,
        programId: TOKEN_2022_PROGRAM_ID,
      }),
      // Name our PDA as the permanent delegate.
      createInitializePermanentDelegateInstruction(
        mint.publicKey,
        delegatePda,
        TOKEN_2022_PROGRAM_ID
      ),
      createInitializeMintInstruction(
        mint.publicKey,
        DECIMALS,
        payer.publicKey, // mint authority
        null, // no freeze authority (P3b is about the delegate, not freeze)
        TOKEN_2022_PROGRAM_ID
      )
    );

    await sendAndConfirmTransaction(connection, tx, [payer, mint]);
  });

  it("initializes config and mints 100 tokens to the victim", async () => {
    await program.methods
      .initializeConfig()
      .accounts({ admin: payer.publicKey })
      .rpc();

    const createAtaTx = new Transaction().add(
      createAssociatedTokenAccountInstruction(
        payer.publicKey,
        victimAta,
        victim.publicKey,
        mint.publicKey,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, createAtaTx, [payer]);

    await mintTo(
      connection,
      payer,
      mint.publicKey,
      victimAta,
      payer, // mint authority
      100 * ONE,
      [],
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    assert.equal((await balance(victimAta)).toString(), (100 * ONE).toString());
  });

  it("claws back 60 tokens from the victim (victim does NOT sign)", async () => {
    // Issuer's recovery account.
    const createAtaTx = new Transaction().add(
      createAssociatedTokenAccountInstruction(
        payer.publicKey,
        recoveryAta,
        payer.publicKey,
        mint.publicKey,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, createAtaTx, [payer]);

    await program.methods
      .clawback(new anchor.BN(60 * ONE))
      .accounts({
        admin: payer.publicKey,
        config: configPda,
        mint: mint.publicKey,
        from: victimAta,
        to: recoveryAta,
        delegate: delegatePda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc(); // note: only the admin signs — the victim keypair is nowhere here

    assert.equal((await balance(victimAta)).toString(), (40 * ONE).toString());
    assert.equal((await balance(recoveryAta)).toString(), (60 * ONE).toString());
  });

  it("burns the victim's remaining 40 tokens (sanctions-style)", async () => {
    const supplyBefore = (await getMint(
      connection,
      mint.publicKey,
      undefined,
      TOKEN_2022_PROGRAM_ID
    )).supply;

    await program.methods
      .burnFrom(new anchor.BN(40 * ONE))
      .accounts({
        admin: payer.publicKey,
        config: configPda,
        mint: mint.publicKey,
        from: victimAta,
        delegate: delegatePda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const supplyAfter = (await getMint(
      connection,
      mint.publicKey,
      undefined,
      TOKEN_2022_PROGRAM_ID
    )).supply;

    assert.equal((await balance(victimAta)).toString(), "0");
    assert.equal(
      (supplyBefore - supplyAfter).toString(),
      (40 * ONE).toString(),
      "burned tokens should reduce total supply"
    );
  });
});
