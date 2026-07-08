// ============================================================================
// P6b test — proves the constant-product AMM lifecycle:
//   token A (tokenized stock) + token B (USDC) + an LP mint (authority = PDA)
//   -> initialize_pool (0.3% fee)
//   -> add_liquidity: 100 A + 200 B  => pool price 2 B/A, LP minted
//   -> swap 10 A -> B                => output follows x*y=k minus fee
//   -> remove_liquidity: burn all LP => get both tokens back
//
// Read alongside lib.rs: the pool holds two reserves; price = reserve_b/reserve_a
// emerges from the balances, and every swap keeps reserve_a*reserve_b ~constant.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P6bMiniAmm } from "../target/types/p6b_mini_amm";
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

describe("p6b-mini-amm", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P6bMiniAmm as Program<P6bMiniAmm>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;
  const payer = wallet.payer;

  const DECIMALS = 6;
  const ONE = 10 ** DECIMALS;
  const FEE_BPS = 30; // 0.3%

  const INIT_A = 100 * ONE;
  const INIT_B = 200 * ONE; // initial price: 2 B per A

  let mintA: PublicKey; // tokenized stock
  let mintB: PublicKey; // USDC
  let lpMint: PublicKey;

  const [poolPda] = PublicKey.findProgramAddressSync([Buffer.from("pool")], program.programId);
  const [authorityPda] = PublicKey.findProgramAddressSync([Buffer.from("authority")], program.programId);
  const [reserveAPda] = PublicKey.findProgramAddressSync([Buffer.from("reserve_a")], program.programId);
  const [reserveBPda] = PublicKey.findProgramAddressSync([Buffer.from("reserve_b")], program.programId);

  let userA: PublicKey;
  let userB: PublicKey;
  let userLp: PublicKey;

  const bal = async (ata: PublicKey) =>
    Number((await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount);

  it("sets up mints and funds the user", async () => {
    mintA = await createMint(connection, payer, payer.publicKey, null, DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID);
    mintB = await createMint(connection, payer, payer.publicKey, null, DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID);
    // LP mint authority = the pool PDA, so only the program mints/burns LP.
    lpMint = await createMint(connection, payer, authorityPda, null, DECIMALS, undefined, undefined, TOKEN_2022_PROGRAM_ID);

    userA = getAssociatedTokenAddressSync(mintA, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    userB = getAssociatedTokenAddressSync(mintB, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    userLp = getAssociatedTokenAddressSync(lpMint, payer.publicKey, false, TOKEN_2022_PROGRAM_ID);
    await sendAndConfirmTransaction(
      connection,
      new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, userA, payer.publicKey, mintA, TOKEN_2022_PROGRAM_ID),
        createAssociatedTokenAccountInstruction(payer.publicKey, userB, payer.publicKey, mintB, TOKEN_2022_PROGRAM_ID),
        createAssociatedTokenAccountInstruction(payer.publicKey, userLp, payer.publicKey, lpMint, TOKEN_2022_PROGRAM_ID)
      ),
      [payer]
    );
    await mintTo(connection, payer, mintA, userA, payer, 1000 * ONE, [], undefined, TOKEN_2022_PROGRAM_ID);
    await mintTo(connection, payer, mintB, userB, payer, 1000 * ONE, [], undefined, TOKEN_2022_PROGRAM_ID);
  });

  it("initializes the pool", async () => {
    await program.methods
      .initializePool(FEE_BPS)
      .accounts({
        admin: payer.publicKey,
        tokenAMint: mintA,
        tokenBMint: mintB,
        lpMint,
        reserveA: reserveAPda,
        reserveB: reserveBPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();
  });

  it("adds the first liquidity (100 A + 200 B)", async () => {
    await program.methods
      .addLiquidity(new anchor.BN(INIT_A), new anchor.BN(INIT_B), new anchor.BN(0))
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        tokenAMint: mintA,
        tokenBMint: mintB,
        lpMint,
        reserveA: reserveAPda,
        reserveB: reserveBPda,
        userA,
        userB,
        userLp,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal(await bal(reserveAPda), INIT_A, "reserve A seeded");
    assert.equal(await bal(reserveBPda), INIT_B, "reserve B seeded");
    assert.isAbove(await bal(userLp), 0, "LP tokens minted to provider");
  });

  it("swaps 10 A -> B along x*y=k (minus fee)", async () => {
    const bBefore = await bal(userB);
    const kBefore = (await bal(reserveAPda)) * (await bal(reserveBPda));

    await program.methods
      .swap(new anchor.BN(10 * ONE), new anchor.BN(1), true) // a_to_b = true
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        tokenAMint: mintA,
        tokenBMint: mintB,
        reserveA: reserveAPda,
        reserveB: reserveBPda,
        userA,
        userB,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    const received = (await bal(userB)) - bBefore;
    console.log(`swapped 10 A -> ${received / ONE} B`);

    assert.equal(await bal(reserveAPda), INIT_A + 10 * ONE, "reserve A grew by input");
    assert.isAbove(received, 0, "got some B out");
    assert.isBelow(received, 20 * ONE, "slippage: less than the naive 2:1 rate");
    // The product must not shrink (the fee actually makes it grow slightly).
    const kAfter = (await bal(reserveAPda)) * (await bal(reserveBPda));
    assert.isAtLeast(kAfter, kBefore, "constant-product invariant holds");
  });

  it("removes all liquidity and returns both tokens", async () => {
    const lp = await bal(userLp);

    await program.methods
      .removeLiquidity(new anchor.BN(lp), new anchor.BN(0), new anchor.BN(0))
      .accounts({
        user: payer.publicKey,
        pool: poolPda,
        tokenAMint: mintA,
        tokenBMint: mintB,
        lpMint,
        reserveA: reserveAPda,
        reserveB: reserveBPda,
        userA,
        userB,
        userLp,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal(await bal(userLp), 0, "all LP burned");
    // Sole provider withdrew everything, so reserves are ~empty (integer dust ok).
    assert.isBelow(await bal(reserveAPda), ONE, "reserve A drained");
    assert.isBelow(await bal(reserveBPda), ONE, "reserve B drained");
  });
});
