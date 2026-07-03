// ============================================================================
// P5a test — proves the fund subscribe/redeem cycle at a fixed $1 NAV:
//   create a mock-USDC "cash" mint + an INTEREST-BEARING share mint
//   -> initialize the fund (records mints, creates the vault)
//   -> user subscribes 1000 cash  => vault holds 1000, user gets 1000 shares
//   -> user redeems  400 shares    => user gets 400 cash back, 600 shares left
//
// Read alongside lib.rs: the program moves RAW token amounts 1:1. The yield is
// carried entirely by the share mint's interest-bearing extension (set up here),
// which grows the *displayed* balance over time without minting new tokens.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P5aTreasuryFund } from "../target/types/p5a_treasury_fund";
import {
  TOKEN_2022_PROGRAM_ID,
  ExtensionType,
  getMintLen,
  createInitializeMintInstruction,
  createInitializeInterestBearingMintInstruction,
  createAssociatedTokenAccountInstruction,
  getAssociatedTokenAddressSync,
  getInterestBearingMintConfigState,
  createMint,
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

describe("p5a-treasury-fund", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P5aTreasuryFund as Program<P5aTreasuryFund>;
  const connection = provider.connection;
  const wallet = provider.wallet as anchor.Wallet;
  const payer = wallet.payer; // acts as admin AND the subscribing user (kept simple)

  const shareMint = Keypair.generate(); // interest-bearing fund share token
  const DECIMALS = 6;
  const ONE = 10 ** DECIMALS;
  const YIELD_RATE_BPS = 500; // 5% APR, carried by the interest-bearing extension

  let depositMint: PublicKey; // mock USDC (plain Token-2022 mint)

  // Program PDAs.
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

  let userDepositAta: PublicKey;
  let userShareAta: PublicKey;

  const bal = async (ata: PublicKey) =>
    (await getAccount(connection, ata, undefined, TOKEN_2022_PROGRAM_ID)).amount;

  it("creates the cash mint and the interest-bearing share mint", async () => {
    // Cash mint (mock USDC): a plain Token-2022 mint, authority = payer.
    depositMint = await createMint(
      connection,
      payer,
      payer.publicKey, // mint authority
      null,
      DECIMALS,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    // Share mint: Token-2022 with the Interest-Bearing extension. Order matters
    // (same rule as P2/P3): allocate -> init extension -> init mint. The mint
    // authority is our program's PDA, so ONLY the program can mint shares.
    const mintLen = getMintLen([ExtensionType.InterestBearingConfig]);
    const lamports = await connection.getMinimumBalanceForRentExemption(mintLen);

    const tx = new Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: shareMint.publicKey,
        space: mintLen,
        lamports,
        programId: TOKEN_2022_PROGRAM_ID,
      }),
      // The yield: rate authority = payer, rate = 5% APR (basis points). This is
      // what makes the share's UI amount grow over time.
      createInitializeInterestBearingMintInstruction(
        shareMint.publicKey,
        payer.publicKey, // rate authority (could later change the rate)
        YIELD_RATE_BPS,
        TOKEN_2022_PROGRAM_ID
      ),
      createInitializeMintInstruction(
        shareMint.publicKey,
        DECIMALS,
        authorityPda, // mint authority = program PDA
        null,
        TOKEN_2022_PROGRAM_ID
      )
    );
    await sendAndConfirmTransaction(connection, tx, [payer, shareMint]);

    // Confirm the interest-bearing config really is on the mint.
    const mintInfo = await getMint(
      connection,
      shareMint.publicKey,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );
    const ib = getInterestBearingMintConfigState(mintInfo);
    assert.equal(ib?.currentRate, YIELD_RATE_BPS, "share mint should accrue 5%");
  });

  it("initializes the fund (records mints, creates the vault)", async () => {
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
  });

  it("funds the user with 1000 cash and creates their share account", async () => {
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

    const tx = new Transaction().add(
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
    );
    await sendAndConfirmTransaction(connection, tx, [payer]);

    await mintTo(
      connection,
      payer,
      depositMint,
      userDepositAta,
      payer, // cash mint authority
      1000 * ONE,
      [],
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    assert.equal((await bal(userDepositAta)).toString(), (1000 * ONE).toString());
  });

  it("subscribes 1000 cash -> 1000 shares (vault custodies the cash)", async () => {
    await program.methods
      .subscribe(new anchor.BN(1000 * ONE))
      .accounts({
        user: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        depositMint,
        userDepositAta,
        vault: vaultPda,
        userShareAta,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal((await bal(userDepositAta)).toString(), "0", "cash left the user");
    assert.equal((await bal(vaultPda)).toString(), (1000 * ONE).toString(), "vault holds cash");
    assert.equal((await bal(userShareAta)).toString(), (1000 * ONE).toString(), "user got shares");
  });

  it("redeems 400 shares -> 400 cash back", async () => {
    await program.methods
      .redeem(new anchor.BN(400 * ONE))
      .accounts({
        user: payer.publicKey,
        config: configPda,
        shareMint: shareMint.publicKey,
        depositMint,
        userDepositAta,
        vault: vaultPda,
        userShareAta,
        authority: authorityPda,
        tokenProgram: TOKEN_2022_PROGRAM_ID,
      })
      .rpc();

    assert.equal((await bal(userShareAta)).toString(), (600 * ONE).toString(), "shares burned");
    assert.equal((await bal(userDepositAta)).toString(), (400 * ONE).toString(), "cash returned");
    assert.equal((await bal(vaultPda)).toString(), (600 * ONE).toString(), "vault paid out");
  });
});
