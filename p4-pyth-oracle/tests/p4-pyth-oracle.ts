// ============================================================================
// P4 test — proves the oracle read end-to-end:
//   the SOL/USD price feed account is cloned from mainnet (see Anchor.toml)
//   -> we DERIVE its address the same way the client would
//   -> call read_price on it
//   -> the program validates staleness + confidence + feed id and logs the price
//
// Read alongside lib.rs: the program is the READER/validator; a real client is
// what POSTS the price. Here Anchor.toml's clone plays the role of "someone
// already posted a fresh SOL/USD update", so we only have to read it.
// ============================================================================

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { P4PythOracle } from "../target/types/p4_pyth_oracle";
import { PythSolanaReceiver } from "@pythnetwork/pyth-solana-receiver";
import { assert } from "chai";

// The SOL/USD feed id (hex) — MUST match SOL_USD_FEED_ID in lib.rs.
const SOL_USD_FEED_ID =
  "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

describe("p4-pyth-oracle", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.P4PythOracle as Program<P4PythOracle>;
  const wallet = provider.wallet as anchor.Wallet;

  // Derive the address of the SOL/USD price feed account (shard 0). This is the
  // deterministic address the receiver SDK computes — and the exact account we
  // cloned from mainnet in Anchor.toml, so it exists in the local validator.
  const receiver = new PythSolanaReceiver({
    connection: provider.connection,
    wallet,
  });
  const solUsdPriceAccount = receiver.getPriceFeedAccountAddress(
    0,
    SOL_USD_FEED_ID
  );

  it("reads and validates the SOL/USD price", async () => {
    const txSig = await program.methods
      .readPrice()
      .accounts({ priceUpdate: solUsdPriceAccount })
      .rpc();

    // Pull the program log so we can see (and assert on) the price it read.
    const tx = await provider.connection.getTransaction(txSig, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    const logs = (tx?.meta?.logMessages ?? []).join("\n");
    console.log(logs);

    assert.match(logs, /SOL\/USD = /, "program should log the validated price");
  });
});
