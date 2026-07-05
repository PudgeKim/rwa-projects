// ============================================================================
// P5b test — proves oracle-priced subscribe/redeem:
//   mock-wSOL "deposit" mint (9 dp) + interest-bearing share mint (6 dp)
//   -> initialize the fund
//   -> user subscribes 10 wSOL  => shares minted = USD value at the SOL/USD price
//   -> user redeems ALL shares  => gets ~10 wSOL back (round-trips at one price)
//
// The assertion is price-AGNOSTIC on purpose: we don't hardcode SOL's price, we
// just check that subscribing then immediately redeeming returns the original
// deposit (minus tiny integer-rounding dust). The SOL/USD account is cloned from
// mainnet in Anchor.toml, so a real, fresh price drives the math.
//
// Read alongside lib.rs: `deposit_to_shares` and `shares_to_deposit` are exact
// inverses, so a round trip at a single price is (almost) lossless.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P5bTreasuryOracle } from "../target/types/p5b_treasury_oracle";
import { PythSolanaReceiver } from "@pythnetwork/pyth-solana-receiver";
import {
  TOKEN_2022_PROGRAM_ID,
  ExtensionType,
  getMintLen,
  createInitializeMintInstruction,
  createInitializeInterestBearingMintInstruction,
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

describe("p5b-treasury-oracle", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace
    .P5bTreasuryOracle as Program<P5bTreasuryOracle>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;
  const payer = wallet.payer; // admin AND user (kept simple)

  const shareMint = Keypair.generate(); // interest-bearing, 6 decimals
  const SHARE_DECIMALS = 6;
  const DEPOSIT_DECIMALS = 9; // wSOL-like
  const ONE_WSOL = 10 ** DEPOSIT_DECIMALS;
  const DEPOSIT = 10 * ONE_WSOL; // subscribe 10 wSOL

  let depositMint: PublicKey; // mock wSOL

  const [configPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("config")],
    program.programId
  );
  const [authorityPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("authority")],
    program.programId
  );
  const [vaultPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    program.programId
  );

  // The cloned SOL/USD price account (derived exactly as a client would).
  const receiver = new PythSolanaReceiver({ connection, wallet });
  const solUsdPriceAccount = receiver.getPriceFeedAccountAddress(
    0,
    SOL_USD_FEED_ID
  );

  let userDepositAta: PublicKey;
  let userShareAta: PublicKey;

  const bal = async (ata: PublicKey) =>
    Number((await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount);

  it("sets up the mints, fund, and funds the user with 10 wSOL", async () => {
    // Deposit mint: mock wSOL (9 decimals), authority = payer.
    depositMint = await createMint(
      connection,
      payer,
      payer.publicKey,
      null,
      DEPOSIT_DECIMALS,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    // Share mint: interest-bearing (6 decimals), authority = program PDA.
    const mintLen = getMintLen([ExtensionType.InterestBearingConfig]);
    const lamports = await connection.getMinimumBalanceForRentExemption(mintLen);
    const mintTx = new Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: shareMint.publicKey,
        space: mintLen,
        lamports,
        programId: TOKEN_2022_PROGRAM_ID,
      }),
      createInitializeInterestBearingMintInstruction(
        shareMint.publicKey,
        payer.publicKey,
        500, // 5% APR
        TOKEN_2022_PROGRAM_ID
      ),
      createInitializeMintInstruction(
        shareMint.publicKey,
        SHARE_DECIMALS,
        authorityPda,
        null,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, mintTx, [payer, shareMint]);

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

    // User's ATAs + 10 wSOL of deposit.
    userDepositAta = getAssociatedTokenAddressSync(
      depositMint,
      payer.publicKey,
      false,
      TOKEN_2022_PROGRAM_ID
    );
    userShareAta = getAssociatedTokenAddressSync(
      shareMint.publicKey,
      payer.publicKey,
      false,
      TOKEN_2022_PROGRAM_ID
    );
    await sendAndConfirmTransaction(
      connection,
      new Transaction().add(
        createAssociatedTokenAccountInstruction(
          payer.publicKey,
          userDepositAta,
          payer.publicKey,
          depositMint,
          TOKEN_2022_PROGRAM_ID
        ),
        createAssociatedTokenAccountInstruction(
          payer.publicKey,
          userShareAta,
          payer.publicKey,
          shareMint.publicKey,
          TOKEN_2022_PROGRAM_ID
        )
      ),
      [payer]
    );
    await mintTo(
      connection,
      payer,
      depositMint,
      userDepositAta,
      payer,
      DEPOSIT,
      [],
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    assert.equal(await bal(userDepositAta), DEPOSIT);
  });

  it("subscribes 10 wSOL and gets USD-valued shares", async () => {
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
        priceUpdate: solUsdPriceAccount,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const shares = await bal(userShareAta);
    console.log(`10 wSOL -> ${shares / 10 ** SHARE_DECIMALS} shares (~USD)`);

    assert.isAbove(shares, 0, "should mint USD-valued shares");
    assert.equal(await bal(userDepositAta), 0, "deposit left the user");
    assert.equal(await bal(vaultPda), DEPOSIT, "vault custodies the wSOL");
  });

  it("redeems all shares and gets ~10 wSOL back (round-trip)", async () => {
    const shares = await bal(userShareAta);

    await program.methods
      .redeem(new anchor.BN(shares))
      .accounts({
        user: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        depositMint,
        userDepositAta,
        vault: vaultPda,
        userShareAta,
        authority: authorityPda,
        priceUpdate: solUsdPriceAccount,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const returned = await bal(userDepositAta);
    // Two floor-divisions (in and out) can shave off a little dust; allow a
    // tiny tolerance rather than asserting exact equality.
    const dust = 10 ** 6; // 0.001 wSOL
    assert.equal(await bal(userShareAta), 0, "all shares burned");
    assert.isAtMost(returned, DEPOSIT, "cannot get back more than deposited");
    assert.isAtLeast(
      returned,
      DEPOSIT - dust,
      "round trip returns ~the original deposit"
    );
  });
});
