// Paiement des retraits Duello via Phantom (USDC sur Solana).
//
// La signature se fait EXCLUSIVEMENT dans l'extension Phantom du navigateur
// (`window.phantom.solana`). Switch ne manipule jamais de cle privee ni de
// seed : il ne construit que la transaction, que Phantom signe et diffuse.
//
// Contrat attendu :
// - l'adresse Solana du membre est fournie par Duello (champ `solanaAddress`
//   du wallet, expose par le backend dans `DuelloBankSnapshot`) ;
// - l'envoi cree le compte ATA USDC du membre s'il n'existe pas encore
//   (frais de creation payes en SOL par le wallet expediteur) ;
// - l'admin doit disposer de USDC (montant envoye) ET d'un peu de SOL
//   (frais de transaction) dans son wallet Phantom.

import {
  Connection,
  PublicKey,
  SystemProgram,
  Transaction,
} from "@solana/web3.js";
import {
  createAssociatedTokenAccountInstruction,
  createTransferCheckedInstruction,
  getAssociatedTokenAddress,
} from "@solana/spl-token";

// USDC mainnet : mint SPL classique, 6 decimales.
export const USDC_MAINNET_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
export const USDC_DECIMALS = 6;

const RPC_ENDPOINT =
  (typeof import.meta !== "undefined" && import.meta.env?.VITE_SOLANA_RPC
    ? String(import.meta.env.VITE_SOLANA_RPC).trim()
    : "") || "https://api.mainnet-beta.solana.com";

const shortenAddress = (address: string) =>
  address.length > 13 ? `${address.slice(0, 4)}…${address.slice(-4)}` : address;

export const formatSolanaAddress = shortenAddress;

// Interface minimale de l'extension Phantom exposee sur `window.phantom.solana`.
type PhantomProvider = {
  isPhantom?: boolean;
  connect: (options?: { onlyIfTrusted?: boolean }) => Promise<{ publicKey: { toString(): string } }>;
  disconnect: () => Promise<void>;
  signAndSendTransaction: (
    transaction: Transaction,
  ) => Promise<{ signature: string }>;
  on?: (event: string, handler: (...args: unknown[]) => void) => void;
};

const phantomProvider = (): PhantomProvider | null => {
  const globalWindow = window as unknown as {
    phantom?: { solana?: PhantomProvider };
  };
  return globalWindow.phantom?.solana ?? null;
};

export const phantomAvailable = () => Boolean(phantomProvider());

export const phantomIsInstalled = () => phantomAvailable();

export const phantomErrorMessage = () => {
  if (typeof window === "undefined") return "Environnement non navigateur.";
  if (!phantomProvider()) {
    return "Extension Phantom introuvable. Installez Phantom (chrome.google.com/webstore) puis rechargez la page.";
  }
  return "";
};

export type PhantomConnection = {
  address: string;
};

export async function connectPhantom(): Promise<PhantomConnection> {
  const provider = phantomProvider();
  if (!provider) throw new Error(phantomErrorMessage());
  try {
    const { publicKey } = await provider.connect();
    const address = walletPublicKey(
      publicKey.toString(),
      "L'adresse Phantom renvoyee est invalide.",
    ).toBase58();
    return { address };
  } catch (error) {
    if (error instanceof Error && /user rejected|declined/i.test(error.message)) {
      throw new Error("Connexion Phantom annulee.");
    }
    throw error;
  }
}

export async function disconnectPhantom(): Promise<void> {
  const provider = phantomProvider();
  if (provider) await provider.disconnect();
}

export type UsdcBalance = {
  /** Solde en unites mineures (6 decimales), 0 si aucun compte ATA. */
  minor: bigint;
  /** Le compte ATA USDC du wallet existe-t-il deja sur la chaine ? */
  ataExists: boolean;
};

export async function getUsdcBalance(address: string): Promise<UsdcBalance> {
  const connection = new Connection(RPC_ENDPOINT, "confirmed");
  const owner = walletPublicKey(address, "Adresse Phantom source invalide.");
  const mint = new PublicKey(USDC_MAINNET_MINT);
  const ata = await getAssociatedTokenAddress(mint, owner);
  const accountInfo = await connection.getAccountInfo(ata);
  if (!accountInfo) return { minor: 0n, ataExists: false };
  try {
    const { value } = await connection.getTokenAccountBalance(ata);
    const minor = BigInt(value.amount);
    return { minor, ataExists: true };
  } catch {
    // Compte present mais illisible (probablement 0) : on le traite comme vide.
    return { minor: 0n, ataExists: true };
  }
}

export const formatUsdcMinor = (minor: bigint) => {
  const negative = minor < 0n;
  const abs = negative ? -minor : minor;
  const units = abs / 10n ** BigInt(USDC_DECIMALS);
  const fraction = abs % 10n ** BigInt(USDC_DECIMALS);
  const formatted = `${units.toLocaleString("fr-FR")},${fraction
    .toString()
    .padStart(USDC_DECIMALS, "0")} USDC`;
  return negative ? `-${formatted}` : formatted;
};

export type SendUsdcRequest = {
  /** Wallet Phantom connecte (expediteur, payer des frais). */
  fromAddress: string;
  /** Adresse Solana du membre destinataire. */
  toAddress: string;
  /** Montant en unites mineures USDC (6 decimales). */
  amountMinor: bigint;
};

export type SendUsdcResult = {
  signature: string;
  transactionUrl: string;
  createdDestinationAta: boolean;
  blockhash: string;
  lastValidBlockHeight: number;
};

const isValidSolanaAddress = (value: string) =>
  /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(value);

const walletPublicKey = (value: string, errorMessage: string) => {
  if (!isValidSolanaAddress(value)) throw new Error(errorMessage);
  try {
    const publicKey = new PublicKey(value);
    // Duello fournit une adresse de wallet, pas un mint, un compte token ou un
    // PDA. Une cle hors courbe ne possede pas de cle privee ed25519.
    if (!PublicKey.isOnCurve(publicKey.toBytes())) throw new Error(errorMessage);
    return publicKey;
  } catch {
    throw new Error(errorMessage);
  }
};

export async function sendUsdc(request: SendUsdcRequest): Promise<SendUsdcResult> {
  const provider = phantomProvider();
  if (!provider) throw new Error(phantomErrorMessage());
  if (request.amountMinor <= 0n) {
    throw new Error("Le montant USDC doit etre positif.");
  }
  if (request.amountMinor > 1_000_000_000_000n) {
    throw new Error("Le montant USDC depasse la limite de 1 000 000 USDC.");
  }

  const connection = new Connection(RPC_ENDPOINT, "confirmed");
  const from = walletPublicKey(request.fromAddress, "Adresse Phantom source invalide.");
  const to = walletPublicKey(request.toAddress, "Adresse Solana du membre invalide.");
  const mint = new PublicKey(USDC_MAINNET_MINT);

  // Si le compte destinataire existe deja, il doit appartenir au System
  // Program. Pour une adresse non financee, la verification on-curve ci-dessus
  // suffit : le compte deviendra un wallet standard a son premier financement.
  const recipientAccount = await connection.getAccountInfo(to);
  if (recipientAccount && !recipientAccount.owner.equals(SystemProgram.programId)) {
    throw new Error(
      "L'adresse Solana du membre n'est pas un wallet standard (compte programme refuse).",
    );
  }

  const fromAta = await getAssociatedTokenAddress(mint, from);
  const toAta = await getAssociatedTokenAddress(mint, to);

  const transaction = new Transaction();
  let createdDestinationAta = false;

  const destinationAccount = await connection.getAccountInfo(toAta);
  if (!destinationAccount) {
    // Le membre n'a jamais recu d'USDC : creer son compte ATA (frais SOL payes
    // par le wallet Phantom expediteur). Cette instruction doit etre signee
    // par le proprietaire du compte de financement, donc par Phantom.
    transaction.add(
      createAssociatedTokenAccountInstruction(from, toAta, to, mint),
    );
    createdDestinationAta = true;
  }

  transaction.add(
    createTransferCheckedInstruction(
      fromAta,
      mint,
      toAta,
      from,
      request.amountMinor,
      USDC_DECIMALS,
    ),
  );

  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash("confirmed");
  transaction.recentBlockhash = blockhash;
  transaction.feePayer = from;

  let signature: string;
  try {
    ({ signature } = await provider.signAndSendTransaction(transaction));
  } catch (error) {
    if (error instanceof Error && /user rejected|declined/i.test(error.message)) {
      throw new Error("Transaction annulee dans Phantom.");
    }
    throw error;
  }
  if (!signature) throw new Error("Phantom n'a pas renvoye de signature.");

  const transactionUrl = `https://solscan.io/tx/${signature}`;
  return {
    signature,
    transactionUrl,
    createdDestinationAta,
    blockhash,
    lastValidBlockHeight,
  };
}

export async function waitForTransactionConfirmation(result: SendUsdcResult): Promise<void> {
  const connection = new Connection(RPC_ENDPOINT, "confirmed");
  const confirmation = await connection.confirmTransaction(
    {
      signature: result.signature,
      blockhash: result.blockhash,
      lastValidBlockHeight: result.lastValidBlockHeight,
    },
    "confirmed",
  );
  if (confirmation.value.err) {
    throw new Error("La transaction USDC a echoue sur la chaine.");
  }
}

export const SOLSCAN_ADDRESS_PREFIX = "https://solscan.io/account/";
