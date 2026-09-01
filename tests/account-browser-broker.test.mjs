import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import {
  ACCOUNT_BROWSER_ACTION_TIMEOUT,
  handleAccountBrowserConnection,
} from "../scripts/account-browser-broker.mjs";
import {
  parseAccountBrowserRequest,
  proxyOptionsFromUrl,
} from "../scripts/open-account-browser.mjs";

const authLink = "https://freebuff.com/login?auth_code=AbCdeFgH1234";

// Socket factice : meme surface que celle utilisee par le broker (setEncoding,
// setTimeout, on, end, destroy), sans lier un vrai pipe nomme.
const fakeSocket = () => {
  const socket = new EventEmitter();
  socket.setEncoding = () => undefined;
  socket.setTimeout = () => undefined;
  socket.destroy = () => undefined;
  socket.ended = "";
  socket.end = (data) => {
    if (data) socket.ended = data;
    socket.emit("reply");
  };
  return socket;
};

const payload = JSON.stringify({ accountId: "compte-123-abcd", url: authLink });

const sendUntilResponse = (socket, chunk) =>
  new Promise((resolve) => {
    socket.once("reply", () => resolve(JSON.parse(socket.ended.trim())));
    socket.emit("data", chunk);
  });

test("le lien officiel de connexion Freebuff est accepte", () => {
  const request = parseAccountBrowserRequest({
    accountId: "compte-123-abcd",
    url: authLink,
    proxyUrl: "socks5://user:p%40ss@127.0.0.2:1080/",
  });
  assert.equal(request.accountId, "compte-123-abcd");
  assert.equal(request.url, authLink);
  assert.deepEqual(request.proxy, {
    server: "socks5://127.0.0.2:1080",
    username: "user",
    password: "p@ss",
  });
});

test("les liens de connexion non officiels sont refuses", () => {
  const invalid = [
    { accountId: "", url: authLink },
    { accountId: "compte-123-abcd", url: "http://freebuff.com/login?auth_code=AbCdeFgH1234" },
    { accountId: "compte-123-abcd", url: "https://freebuff.com/register?auth_code=AbCdeFgH1234" },
    { accountId: "compte-123-abcd", url: "https://autre-site.com/login?auth_code=AbCdeFgH1234" },
    { accountId: "compte-123-abcd", url: "https://freebuff.com/login" },
  ];
  for (const value of invalid) {
    assert.throws(() => parseAccountBrowserRequest(value));
  }
  assert.throws(() => proxyOptionsFromUrl("ftp://127.0.0.2:21/"));
});

test("le broker repond au succes via le runner injecte", async () => {
  const socket = fakeSocket();
  handleAccountBrowserConnection({
    socket,
    runner: async () => ({ ok: true, mode: "proxy" }),
    requestTimeoutMilliseconds: 200,
  });
  const result = await sendUntilResponse(socket, payload + "\n");
  assert.deepEqual(result, { ok: true, mode: "proxy" });
});

test("une action qui depasse le delai repond en erreur au lieu de bloquer", async () => {
  const socket = fakeSocket();
  const started = Date.now();
  handleAccountBrowserConnection({
    socket,
    runner: () => new Promise(() => undefined),
    requestTimeoutMilliseconds: 150,
  });
  const result = await sendUntilResponse(socket, payload + "\n");
  assert.equal(result.ok, false);
  assert.match(result.error, /trop de temps/);
  assert.ok(Date.now() - started < 5000, "le chien de garde doit couper l'action");
});

test("une demande illisible repond en erreur", async () => {
  const socket = fakeSocket();
  handleAccountBrowserConnection({ socket, runner: async () => ({ ok: true }) });
  const result = await sendUntilResponse(socket, "pas du json\n");
  assert.equal(result.ok, false);
});

test("le delai par defaut couvre lancement Chrome + navigation proxy", () => {
  assert.ok(ACCOUNT_BROWSER_ACTION_TIMEOUT >= 90_000);
});