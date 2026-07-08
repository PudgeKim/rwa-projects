// ============================================================================
// P6a test — proves oracle-priced stock issuance:
//   mock-USDC mint (6 dp) + tokenized-stock mint (6 dp, authority = PDA)
//   -> initialize the market
//   -> user BUYS 2 shares  => pays SOL/USD-priced USDC, receives 2 shares
//   -> user SELLS 2 shares  => gets exactly that USDC back (same price)
//
// The round trip is price-AGNOSTIC: buy cost and sell proceeds are the same
// function of (shares, price), so selling right back returns the exact USDC.
// The SOL/USD account (a 24/7 stand-in for a share feed) is cloned from mainnet.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P6aTokenizedStock } from "../target/types/p6a_tokenized_stock";
import { PythSolanaReceiver } from "@pythnetwork/pyth-solana-receiver";
import {
  TOKEN_2022_PROGRAM_ID,
  createAssociatedTokenAccountInstruction,
  getAssociatedTokenAddressSync,
  createMint,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import {
  PublicKey,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { assert } from "chai";

const SHARE_PRICE_FEED_ID =
  "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

describe("p6a-tokenized-stock", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace
    .P6aTokenizedStock as Program<P6aTokenizedStock>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;
  const payer = wallet.payer; // admin AND user

  const DECIMALS = 6;
  const ONE = 10 ** DECIMALS;
  const SHARES = 2 * ONE; // buy 2 whole shares
  const USDC_FUNDING = 100_000 * ONE;

  let stockMint: PublicKey;
  let usdcMint: PublicKey;

  const [marketPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("market")],
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

  const receiver = new PythSolanaReceiver({ connection, wallet });
  const priceAccount = receiver.getPriceFeedAccountAddress(0, SHARE_PRICE_FEED_ID);

  let userStockAta: PublicKey;
  let userUsdcAta: PublicKey;

  const bal = async (ata: PublicKey) =>
    Number((await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount);

  it("sets up mints, market, and funds the user with USDC", async () => {
    // Tokenized stock: authority = program PDA so only the program mints shares.
    stockMint = await createMint(
      connection, payer, authorityPda, null, DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID
    );
    // Mock USDC: authority = payer.
    usdcMint = await createMint(
      connection, payer, payer.publicKey, null, DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID
    );

    await program.methods
      .initializeMarket()
      .accounts({
        admin: payer.publicKey,
        stockMint,
        usdcMint,
        vault: vaultPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    userStockAta = getAssociatedTokenAddressSync(stockMint, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    userUsdcAta = getAssociatedTokenAddressSync(usdcMint, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    await sendAndConfirmTransaction(
      connection,
      new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, userStockAta, payer.publicKey, stockMint, TOKEN_2022_PROGRAM_ID),
        createAssociatedTokenAccountInstruction(payer.publicKey, userUsdcAta, payer.publicKey, usdcMint, TOKEN_2022_PROGRAM_ID)
      ),
      [payer]
    );
    await mintTo(connection, payer, usdcMint, userUsdcAta, payer, USDC_FUNDING, [], undefined, TOKEN_2022_PROGRAM_ID);

    assert.equal(await bal(userUsdcAta), USDC_FUNDING);
  });

  let costPaid = 0;

  it("buys 2 shares at the oracle price", async () => {
    const usdcBefore = await bal(userUsdcAta);

    await program.methods
      .buy(new anchor.BN(SHARES))
      .accounts({
        user: payer.publicKey,
        market: marketPda,
        stockMint,
        usdcMint,
        userStockAta,
        userUsdcAta,
        vault: vaultPda,
        authority: authorityPda,
        priceUpdate: priceAccount,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    costPaid = usdcBefore - (await bal(userUsdcAta));
    console.log(`2 shares cost ${costPaid / ONE} USDC (~$${(costPaid / ONE / 2).toFixed(2)}/share)`);

    assert.equal(await bal(userStockAta), SHARES, "received 2 shares");
    assert.isAbove(costPaid, 0, "paid a positive USDC cost");
    assert.equal(await bal(vaultPda), costPaid, "vault holds the proceeds");
  });

  it("sells the 2 shares back for the same USDC", async () => {
    const usdcBefore = await bal(userUsdcAta);

    await program.methods
      .sell(new anchor.BN(SHARES))
      .accounts({
        user: payer.publicKey,
        market: marketPda,
        stockMint,
        usdcMint,
        userStockAta,
        userUsdcAta,
        vault: vaultPda,
        authority: authorityPda,
        priceUpdate: priceAccount,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal(await bal(userStockAta), 0, "shares burned");
    assert.equal(await bal(userUsdcAta) - usdcBefore, costPaid, "got the same USDC back");
    assert.equal(await bal(vaultPda), 0, "vault emptied");
  });
});
