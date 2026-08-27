import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("the Switch shell exposes a lazy Banque Duello tab", async () => {
  const main = await source("src/main.ts");

  assert.match(main, /type AppView =[^;]*\| "duello-bank"/s);
  assert.match(main, /import\("\.\/duello-bank"\)/);
  assert.match(main, /data-view="duello-bank"/);
  assert.match(main, /id="duelloBankToggle"/);
  assert.match(main, /renderDuelloBankPanel/);
  assert.match(main, /refreshDuelloBankPanel/);
});

test("the remote bridge maps the two narrow bank endpoints", async () => {
  const platform = await source("src/platform.ts");

  assert.match(platform, /case "duello_bank_snapshot"[\s\S]*?"GET", "\/api\/duello-bank"/);
  assert.match(platform, /case "credit_duello_wallet"[\s\S]*?"POST", "\/api\/duello-bank\/credits"/);
});

test("financial routes require the exact maintenance token and bounded JSON", async () => {
  const server = await source("src-tauri/src/server.rs");
  const lib = await source("src-tauri/src/lib.rs");

  assert.match(lib, /mod duello_bank;/);
  assert.match(server, /"\/duello-bank", get\(api_duello_bank_snapshot\)/);
  assert.match(server, /"\/duello-bank\/credits"[\s\S]*DefaultBodyLimit::max\(8 \* 1024\)/);
  assert.match(server, /api_duello_bank_snapshot[\s\S]*?check_maintenance_header/);
  assert.match(server, /api_credit_duello_wallet[\s\S]*?check_maintenance_header/);
  assert.match(server, /api_credit_duello_wallet[\s\S]*?websocket_origin_allowed/);
  assert.match(server, /CST_DUELLO_BANK_ADMIN_TOKEN/);
  assert.match(server, /SERVER_CONTROL_SECRET_ENV_VARS[\s\S]*CST_DUELLO_BANK_ADMIN_TOKEN/);
});

test("the Duello proxy fails closed and never relays Stripe account identifiers", async () => {
  const bank = await source("src-tauri/src/duello_bank.rs");

  assert.match(bank, /Policy::none\(\)/);
  assert.match(bank, /url\.scheme\(\) != "https"[\s\S]*loopback/);
  assert.match(bank, /MAX_WALLETS_RESPONSE_BYTES/);
  assert.match(bank, /MAX_CREDIT_RESPONSE_BYTES/);
  assert.match(bank, /deny_unknown_fields/);
  assert.match(bank, /clickCountSnapshot/);
  assert.match(bank, /Idempotency-Key/);
  assert.match(bank, /Sha256::digest/);
  assert.doesNotMatch(bank, /pub stripe_account_id/);
  assert.match(bank, /assert!\(!serialized\.contains\("stripeAccountId"\)\)/);
});

test("the panel separates ledger credit from real Stripe funding", async () => {
  const view = await source("src/duello-bank.ts");

  assert.match(view, /grand livre Duello/i);
  assert.match(view, /Stripe/i);
  assert.match(view, /approvision/i);
  assert.match(view, /confirmation/i);
  assert.match(view, /amountMinor/);
  assert.match(view, /reference/);
  assert.doesNotMatch(view, /STRIPE_SECRET_KEY|CST_DUELLO_BANK_ADMIN_TOKEN/);
});

test("Phantom USDC payments validate the wallet and preserve confirmation state", async () => {
  const payment = await source("src/phantom-pay.ts");
  const view = await source("src/duello-bank.ts");
  const backend = await source("src-tauri/src/duello_bank.rs");

  assert.match(payment, /PublicKey\.isOnCurve/);
  assert.match(payment, /SystemProgram\.programId/);
  assert.match(payment, /createAssociatedTokenAccountInstruction/);
  assert.match(payment, /createTransferCheckedInstruction/);
  assert.match(payment, /lastValidBlockHeight/);
  assert.match(payment, /confirmTransaction/);
  assert.match(backend, /bs58::decode/);
  assert.match(backend, /decoded\.len\(\) != 32/);
  assert.match(view, /payConfirmation = "broadcast"/);
  assert.match(view, /payConfirmation = "confirmed"/);
  assert.match(view, /Vérifiez la signature sur Solscan avant toute nouvelle tentative/);
  assert.doesNotMatch(view, /payResult \? "USDC envoyés"/);
});
