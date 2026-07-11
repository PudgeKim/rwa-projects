// ============================================================================
// P7 capstone test — the full compliance lifecycle on ONE token:
//   share mint = Default-Frozen + Interest-Bearing + Permanent Delegate
//   -> user's share account is created FROZEN (not yet KYC'd)
//   -> subscribe BEFORE KYC  => FAILS (can't mint into a frozen account)
//   -> verify_kyc thaws it    => subscribe now SUCCEEDS (oracle-priced shares)
//   -> clawback               => issuer force-moves shares to a recovery account
//
// This is P2 (freeze/KYC) + P5a (interest-bearing) + P3b (permanent delegate) +
// P4/P5b (oracle pricing) all on one mint, driven by one PDA.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P7CompliantRwa } from "../target/types/p7_compliant_rwa";
import { PythSolanaReceiver } from "@pythnetwork/pyth-solana-receiver";
import {
  TOKEN_2022_PROGRAM_ID,
  ExtensionType,
  AccountState,
  getMintLen,
  createInitializeMintInstruction,
  createInitializeDefaultAccountStateInstruction,
  createInitializeInterestBearingMintInstruction,
  createInitializePermanentDelegateInstruction,
  createAssociatedTokenAccountInstruction,
  getAssociatedTokenAddressSync,
  createMint,
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

const SOL_USD_FEED_ID =
  "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

describe("p7-compliant-rwa", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P7CompliantRwa as Program<P7CompliantRwa>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;
  const payer = wallet.payer; // admin AND the subscribing user

  const shareMint = Keypair.generate();
  const recovery = Keypair.generate(); // issuer's recovery-account owner
  const SHARE_DECIMALS = 6;
  const DEPOSIT_DECIMALS = 9;
  const ONE_WSOL = 10 ** DEPOSIT_DECIMALS;
  const DEPOSIT = 5 * ONE_WSOL;

  let depositMint: PublicKey;

  const [configPda] = PublicKey.findProgramAddressSync([Buffer.from("config")], program.programId);
  const [authorityPda] = PublicKey.findProgramAddressSync([Buffer.from("authority")], program.programId);
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault")], program.programId);

  const receiver = new PythSolanaReceiver({ connection, wallet });
  const priceAccount = receiver.getPriceFeedAccountAddress(0, SOL_USD_FEED_ID);

  let userDepositAta: PublicKey;
  let userShareAta: PublicKey;
  let recoveryShareAta: PublicKey;

  const bal = async (ata: PublicKey) =>
    Number((await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount);

  it("sets up the compliant mint (3 extensions), the fund, and the user", async () => {
    // Deposit mint: mock wSOL.
    depositMint = await createMint(
      connection, payer, payer.publicKey, null, DEPOSIT_DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID
    );

    // Share mint: Default-Frozen + Interest-Bearing + Permanent-Delegate. All
    // authorities (mint, freeze, permanent delegate) = the program's PDA.
    const mintLen = getMintLen([
      ExtensionType.DefaultAccountState,
      ExtensionType.InterestBearingConfig,
      ExtensionType.PermanentDelegate,
    ]);
    const lamports = await connection.getMinimumBalanceForRentExemption(mintLen);
    const tx = new Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: shareMint.publicKey,
        space: mintLen,
        lamports,
        programId: TOKEN_2022_PROGRAM_ID,
      }),
      createInitializeDefaultAccountStateInstruction(
        shareMint.publicKey, AccountState.Frozen, TOKEN_2022_PROGRAM_ID
      ),
      createInitializeInterestBearingMintInstruction(
        shareMint.publicKey, payer.publicKey, 500, TOKEN_2022_PROGRAM_ID
      ),
      createInitializePermanentDelegateInstruction(
        shareMint.publicKey, authorityPda, TOKEN_2022_PROGRAM_ID
      ),
      createInitializeMintInstruction(
        shareMint.publicKey, SHARE_DECIMALS,
        authorityPda, // mint authority
        authorityPda, // freeze authority (needed to thaw on KYC)
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, tx, [payer, shareMint]);

    await program.methods
      .initializeFund()
      .accounts({
        admin: payer.publicKey,
        shareMint: shareMint.publicKey,
        depositMint,
        vault: vaultPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    // User accounts. The share ATA is created FROZEN (default state).
    userDepositAta = getAssociatedTokenAddressSync(depositMint, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    userShareAta = getAssociatedTokenAddressSync(shareMint.publicKey, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    await sendAndConfirmTransaction(
      connection,
      new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, userDepositAta, payer.publicKey, depositMint, TOKEN_2022_PROGRAM_ID),
        createAssociatedTokenAccountInstruction(payer.publicKey, userShareAta, payer.publicKey, shareMint.publicKey, TOKEN_2022_PROGRAM_ID)
      ),
      [payer]
    );
    await mintTo(connection, payer, depositMint, userDepositAta, payer, DEPOSIT, [], undefined, TOKEN_2022_PROGRAM_ID);

    // The share account should start frozen.
    const acct = await getAccount(connection, userShareAta, undefined, TOKEN_2022_PROGRAM_ID);
    assert.isTrue(acct.isFrozen, "new share account is frozen until KYC");
  });

  it("REJECTS subscribe before KYC (share account frozen)", async () => {
    let failed = false;
    try {
      await program.methods
        .subscribe(new anchor.BN(DEPOSIT))
        .accounts({
          user: payer.publicKey,
          config: configPda,
          shareMint: shareMint.publicKey,
          depositMint,
          userDepositAta,
          vault: vaultPda,
          userShareAta,
          authority: authorityPda,
          priceUpdate: priceAccount,
          tokenProgram: TOKEN_2022_PROGRAM_ID,
        })
        .rpc();
    } catch {
      failed = true; // expected: minting into a frozen account fails
    }
    assert.isTrue(failed, "subscribe must fail before KYC");
  });

  it("verify_kyc thaws the account, then subscribe SUCCEEDS", async () => {
    await program.methods
      .verifyKyc()
      .accounts({
        admin: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        userShareAccount: userShareAta,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    await program.methods
      .subscribe(new anchor.BN(DEPOSIT))
      .accounts({
        user: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        depositMint,
        userDepositAta,
        vault: vaultPda,
        userShareAta,
        authority: authorityPda,
        priceUpdate: priceAccount,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const shares = await bal(userShareAta);
    console.log(`5 wSOL -> ${shares / 10 ** SHARE_DECIMALS} shares (~USD)`);
    assert.isAbove(shares, 0, "KYC'd user received shares");
    assert.equal(await bal(vaultPda), DEPOSIT, "vault custodies the deposit");
  });

  it("claws back half the shares to a recovery account", async () => {
    // Recovery account (issuer-controlled) must itself be KYC'd/thawed to hold.
    recoveryShareAta = getAssociatedTokenAddressSync(shareMint.publicKey, recovery.publicKey, false, TOKEN_2022_PROGRAM_ID);
    await sendAndConfirmTransaction(
      connection,
      new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, recoveryShareAta, recovery.publicKey, shareMint.publicKey, TOKEN_2022_PROGRAM_ID)
      ),
      [payer]
    );
    await program.methods
      .verifyKyc()
      .accounts({
        admin: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        userShareAccount: recoveryShareAta,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const before = await bal(userShareAta);
    const clawAmount = Math.floor(before / 2);

    await program.methods
      .clawback(new anchor.BN(clawAmount))
      .accounts({
        admin: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        from: userShareAta, // holder does NOT sign
        to: recoveryShareAta,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal(await bal(recoveryShareAta), clawAmount, "recovery got the clawed shares");
    assert.equal(await bal(userShareAta), before - clawAmount, "holder debited without signing");
  });
});
