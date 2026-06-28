// ============================================================================
// P3a test — proves the transfer-hook allowlist end-to-end:
//   create a Token-2022 mint whose transfer hook = our program
//   -> set up white-list + ExtraAccountMetaList
//   -> mint to a source account
//   -> transfer to a NON-allowlisted wallet => FAILS (hook rejects)
//   -> add that wallet to the white-list
//   -> transfer again => SUCCEEDS
//
// Read alongside lib.rs: the program is the rule-checker; THIS file is the
// issuer/client that creates the mint pointing at the hook and moves tokens.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P3TransferHook } from "../target/types/p3_transfer_hook";
import {
  TOKEN_2022_PROGRAM_ID,
  ExtensionType,
  getMintLen,
  createInitializeMintInstruction,
  createInitializeTransferHookInstruction,
  createAssociatedTokenAccountInstruction,
  getAssociatedTokenAddressSync,
  createTransferCheckedWithTransferHookInstruction,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import {
  PublicKey,
  Keypair,
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { assert } from "chai";

describe("p3-transfer-hook", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P3TransferHook as Program<P3TransferHook>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet; // payer + mint authority + list authority
  const payer = wallet.payer;

  const mint = Keypair.generate(); // the Token-2022 mint (transfer-hook enabled)
  const recipient = Keypair.generate(); // a wallet that will (eventually) be allowed
  const DECIMALS = 6;
  const ONE = 10 ** DECIMALS;

  // PDAs owned by our hook program.
  const [whiteListPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("white_list")],
    program.programId
  );
  const [extraAccountMetaListPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("extra-account-metas"), mint.publicKey.toBuffer()],
    program.programId
  );

  // Token accounts (ATAs) for the Token-2022 mint.
  const sourceAta = getAssociatedTokenAddressSync(
    mint.publicKey,
    payer.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID
  );
  const destinationAta = getAssociatedTokenAddressSync(
    mint.publicKey,
    recipient.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID
  );

  const balance = async (ata: PublicKey) =>
    (await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount;

  it("creates a Token-2022 mint whose transfer hook is our program", async () => {
    // Order matters (same rule as P2): allocate -> init extension -> init mint.
    const mintLen = getMintLen([ExtensionType.TransferHook]);
    const lamports = await connection.getMinimumBalanceForRentExemption(mintLen);

    const tx = new Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: mint.publicKey,
        space: mintLen,
        lamports,
        programId: TOKEN_2022_PROGRAM_ID,
      }),
      // Point the mint's transfer hook at OUR program.
      createInitializeTransferHookInstruction(
        mint.publicKey,
        payer.publicKey, // hook authority (could later change the hook)
        program.programId, // the hook program id
        TOKEN_2022_PROGRAM_ID
      ),
      createInitializeMintInstruction(
        mint.publicKey,
        DECIMALS,
        payer.publicKey, // mint authority
        null, // no freeze authority (this project is about the hook, not freeze)
        TOKEN_2022_PROGRAM_ID
      )
    );

    await sendAndConfirmTransaction(connection, tx, [payer, mint]);
  });

  it("initializes the white-list and the ExtraAccountMetaList", async () => {
    await program.methods
      .initializeWhiteList()
      .accounts({ authority: payer.publicKey })
      .rpc();

    await program.methods
      .initializeExtraAccountMetaList()
      .accounts({
        payer: payer.publicKey,
        extraAccountMetaList: extraAccountMetaListPda,
        mint: mint.publicKey,
      })
      .rpc();

    const list = await program.account.whiteList.fetch(whiteListPda);
    assert.equal(list.wallets.length, 0, "white-list starts empty");
  });

  it("mints tokens to the source account", async () => {
    // Create the payer's ATA, then mint 100 tokens into it.
    const createAtaTx = new Transaction().add(
      createAssociatedTokenAccountInstruction(
        payer.publicKey,
        sourceAta,
        payer.publicKey,
        mint.publicKey,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, createAtaTx, [payer]);

    await mintTo(
      connection,
      payer,
      mint.publicKey,
      sourceAta,
      payer, // mint authority
      100 * ONE,
      [],
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    assert.equal((await balance(sourceAta)).toString(), (100 * ONE).toString());
  });

  it("creates the recipient's token account (allowed — hook only fires on transfer)", async () => {
    const tx = new Transaction().add(
      createAssociatedTokenAccountInstruction(
        payer.publicKey,
        destinationAta,
        recipient.publicKey,
        mint.publicKey,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, tx, [payer]);
  });

  it("FAILS to transfer to a non-allowlisted wallet (hook rejects)", async () => {
    let failed = false;
    try {
      // This helper reads the on-chain ExtraAccountMetaList and auto-appends the
      // white-list account, then Token-2022 CPIs into our hook — which rejects.
      const ix = await createTransferCheckedWithTransferHookInstruction(
        connection,
        sourceAta,
        mint.publicKey,
        destinationAta,
        payer.publicKey,
        BigInt(10 * ONE),
        DECIMALS,
        [],
        undefined,
        TOKEN_2022_PROGRAM_ID
      );
      await sendAndConfirmTransaction(connection, new Transaction().add(ix), [payer]);
    } catch {
      failed = true; // expected: ReceiverNotWhiteListed
    }
    assert.isTrue(failed, "transfer to a non-allowlisted wallet must fail");
  });

  it("adds the wallet to the white-list, then transfer SUCCEEDS", async () => {
    await program.methods
      .addToWhiteList(recipient.publicKey)
      .accounts({ authority: payer.publicKey, whiteList: whiteListPda })
      .rpc();

    const ix = await createTransferCheckedWithTransferHookInstruction(
      connection,
      sourceAta,
      mint.publicKey,
      destinationAta,
      payer.publicKey,
      BigInt(10 * ONE),
      DECIMALS,
      [],
      undefined,
      TOKEN_2022_PROGRAM_ID
    );
    await sendAndConfirmTransaction(connection, new Transaction().add(ix), [payer]);

    assert.equal((await balance(destinationAta)).toString(), (10 * ONE).toString());
    assert.equal((await balance(sourceAta)).toString(), (90 * ONE).toString());
  });
});
