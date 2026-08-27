import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const auth = read("../src/user-auth.ts");
const microsoft = read("../src/microsoft.ts");
const main = read("../src/main.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

test("le contrat AuthUser et AuthConfig sont explicites", () => {
  assert.match(auth, /export type AuthUser = \{/);
  assert.match(auth, /id: string;/);
  assert.match(auth, /username: string;/);
  assert.match(auth, /email: string;/);
  assert.match(auth, /avatarUrl\?: string \| null;/);
  assert.match(auth, /hasPassword: boolean;/);
  assert.match(auth, /googleLinked: boolean;/);
  assert.match(auth, /createdAt: number;/);
  assert.match(auth, /updatedAt: number;/);
  assert.match(auth, /type AuthConfig = \{/);
  assert.match(auth, /enabled: boolean;/);
  assert.match(auth, /registrationEnabled: boolean;/);
  assert.match(auth, /googleEnabled: boolean;/);
  assert.match(auth, /googleLoginUrl\?: string \| null;/);
  assert.match(auth, /type SessionResponse = \{ user: AuthUser \};/);
  assert.match(auth, /export type AuthBootstrapState = "authenticated" \| "required" \| "unsupported";/);
  assert.match(auth, /class AuthApiError extends Error/);
  assert.match(auth, /readonly status: number/);
  assert.ok(auth.includes("AuthApiError"), "erreur API typée");
  assert.ok(auth.includes("AuthBootstrapState"), "état de bootstrap");
});

test("l'état d'authentification est module-local et bien initialisé", () => {
  assert.match(auth, /let authConfig: AuthConfig \| null = null;/);
  assert.match(auth, /let currentUser: AuthUser \| null = null;/);
  assert.match(auth, /let authMode: "login" \| "register" = "login";/);
  assert.match(auth, /let authGateError: string \| null = null;/);
  assert.match(auth, /let profileOpen = false;/);
  assert.match(auth, /let profileError: string \| null = null;/);
  assert.match(auth, /let profileSuccess: string \| null = null;/);
  assert.match(auth, /let accountDeletionError: string \| null = null;/);
  assert.match(auth, /let accountDeletionBusy = false;/);
});

test("les erreurs de connexion reparables sont reconnues et les autres pas", () => {
  const detector = block(auth, "const isRepairableConnectionError =", "const escapeHtml");
  assert.match(detector, /failed to fetch\|networkerror\|network request failed\|fetch failed\|load failed/);
  assert.match(detector, /\/i\.test\(error\)/);
  assert.match(detector, /Boolean\(error &&/);
  assert.ok(detector.includes("|"), "alternatives d'erreurs");
  assert.ok(detector.includes("(error: string | null)"), "entrée nullable");
});

test("escapeHtml et escapeAttr neutralisent les 4 caractères HTML critiques", () => {
  const escape = block(auth, "const escapeHtml =", "const escapeAttr");
  assert.match(escape, /replaceAll\("&", "&amp;"\)/);
  assert.match(escape, /replaceAll\("<", "&lt;"\)/);
  assert.match(escape, /replaceAll\(">", "&gt;"\)/);
  assert.match(escape, /replaceAll\('"', "&quot;"\)/);
  assert.ok(escape.indexOf("&amp;") >= 0, "amp");
  assert.ok(escape.indexOf("&lt;") >= 0, "lt");
  assert.ok(escape.indexOf("&gt;") >= 0, "gt");
  assert.ok(escape.indexOf("&quot;") >= 0, "quot");
  assert.match(auth, /const escapeAttr = escapeHtml;/);
});

test("remoteHostLabel ne sort jamais d'exception", () => {
  const label = block(auth, "const remoteHostLabel =", "const googleLoginUrl");
  assert.match(label, /new URL\(remoteBaseUrl\(\)\)\.host/);
  assert.match(label, /try \{/);
  assert.match(label, /catch \{/);
  assert.match(label, /return remoteBaseUrl\(\);/);
});

test("googleLoginUrl valide l'URL configurée et retombe sur la route du serveur", () => {
  const url = block(auth, "const googleLoginUrl =", "const authApi");
  assert.match(url, /authConfig\?\.googleLoginUrl\?\.trim\(\)/);
  assert.match(url, /new URL\(configured\)/);
  assert.match(url, /url\.protocol === "https:" \|\| url\.protocol === "http:"/);
  assert.match(url, /return url\.href/);
  assert.match(url, /catch \{/);
  assert.match(url, /`\$\{remoteBaseUrl\(\)\}\/api\/auth\/google\/start`/);
  assert.ok(url.indexOf("google/start") >= 0, "route google start");
});

test("authApi enveloppe fetch avec credentials et erreurs typées", () => {
  const api = block(auth, "const authApi =", "const consumeOAuthResult");
  assert.match(api, /fetch\(`\$\{remoteBaseUrl\(\)\}\/api\/auth\$\{path\}`, \{/);
  assert.match(api, /credentials: "include"/);
  assert.match(api, /method: options\.method \?\? "GET"/);
  assert.match(api, /headers: options\.body === undefined \? \{\} : \{ "Content-Type": "application\/json" \}/);
  assert.match(api, /body: options\.body === undefined \? undefined : JSON\.stringify\(options\.body\)/);
  assert.match(api, /response\.text\(\)/);
  assert.match(api, /JSON\.parse\(text\)/);
  assert.match(api, /catch \{\s*value = null;/);
  assert.match(api, /if \(!response\.ok\)/);
  assert.match(api, /value\?\.error\?\.message \|\| value\?\.message \|\| response\.statusText \|\| "Erreur d'authentification"/);
  assert.match(api, /new AuthApiError\(/);
  assert.match(api, /response\.status,/);
});

test("consumeOAuthResult consomme Google et Microsoft sur la même page", () => {
  const consume = block(auth, "const consumeOAuthResult", "export const initializeUserAuth");
  assert.match(consume, /consumeMicrosoftOAuthResult\(\);/);
  assert.match(consume, /url\.searchParams\.get\("auth_error"\)/);
  assert.match(consume, /authGateError = oauthError;/);
  assert.match(consume, /url\.searchParams\.get\("auth"\) === "google"/);
  assert.match(consume, /Connexion Google réussie\./);
  assert.match(consume, /url\.searchParams\.has\("auth"\) \|\| url\.searchParams\.has\("auth_error"\)/);
  assert.match(consume, /url\.searchParams\.delete\("auth"\);/);
  assert.match(consume, /url\.searchParams\.delete\("auth_error"\);/);
  assert.match(consume, /history\.replaceState\(\{\}, "", `\$\{url\.pathname\}\$\{url\.search\}\$\{url\.hash\}`\)/);
  assert.ok(consume.indexOf("new URL(window.location.href)") >= 0, "lit l'URL courante");
});

test("initializeUserAuth gère 404, config vide, session et 401", () => {
  const init = block(auth, "export const initializeUserAuth", "const googleMark");
  assert.match(init, /consumeOAuthResult\(\);/);
  assert.match(init, /authApi<AuthConfig>\("\/config"\)/);
  assert.match(init, /error instanceof AuthApiError && error\.status === 404/);
  assert.match(init, /return "unsupported";/);
  assert.match(init, /typeof authConfig\.enabled !== "boolean"/);
  assert.match(init, /!authConfig \|\| typeof authConfig\.enabled !== "boolean" \|\| !authConfig\.enabled/);
  assert.match(init, /authApi<SessionResponse>\("\/me"\)/);
  assert.match(init, /currentUser = session\.user;/);
  assert.match(init, /return "authenticated";/);
  assert.match(init, /error\.status === 401/);
  assert.match(init, /currentUser = null;/);
  assert.match(init, /return "required";/);
  assert.ok((init.match(/return "unsupported"/g) ?? []).length >= 2, "deux chemins unsupported");
});

test("le logo Google est un SVG 4 couleurs standard", () => {
  const mark = block(auth, "const googleMark", "const authGateMarkup");
  assert.match(mark, /viewBox="0 0 24 24"/);
  assert.match(mark, /aria-hidden="true"/);
  assert.match(mark, /#4285F4/);
  assert.match(mark, /#34A853/);
  assert.match(mark, /#FBBC05/);
  assert.match(mark, /#EA4335/);
  const paths = mark.match(/<path /g) ?? [];
  assert.equal(paths.length, 4, "4 chemins Google");
  assert.ok(mark.indexOf("<svg") >= 0, "svg");
  assert.ok(mark.indexOf("</svg>") >= 0, "fermeture svg");
});

test("la porte de connexion rend login et register avec leurs champs", () => {
  const gate = block(auth, "const authGateMarkup", "export const renderUserAuthGate");
  assert.match(gate, /account-auth/);
  assert.match(gate, /aria-labelledby="accountAuthTitle"/);
  assert.match(gate, /Codex Switch Terminal/);
  assert.match(gate, /Content de vous revoir/);
  assert.match(gate, /Créer votre compte/);
  assert.match(gate, /data-auth-mode="login"/);
  assert.match(gate, /data-auth-mode="register"/);
  assert.match(gate, /role="tablist"/);
  assert.match(gate, /authUsername/);
  assert.match(gate, /minlength="3"/);
  assert.match(gate, /maxlength="32"/);
  assert.match(gate, /pattern="\[A-Za-z0-9_.-\]\+"/);
  assert.match(gate, /authEmail/);
  assert.match(gate, /type="email"/);
  assert.match(gate, /authIdentifier/);
  assert.match(gate, /authPassword/);
  assert.match(gate, /type="password"/);
  assert.match(gate, /new-password/);
  assert.match(gate, /current-password/);
  assert.match(gate, /minlength="10"/);
  assert.match(gate, /authPasswordConfirm/);
  assert.match(gate, /10 caractères minimum/);
  assert.match(gate, /Créer mon compte/);
  assert.match(gate, /Se connecter/);
  assert.match(gate, /account-google-button/);
  assert.match(gate, /Continuer avec Google/);
  assert.match(gate, /Connexion sécurisée/);
  assert.ok(gate.indexOf("account-auth-repair") >= 0, "panneau de réparation");
  assert.ok(gate.indexOf("Reparer la connexion") >= 0, "bouton réparation");
});

test("renderUserAuthGate câble réparation, onglets et soumission", () => {
  const render = block(auth, "export const renderUserAuthGate", "const userInitials");
  assert.match(render, /host\.innerHTML = authGateMarkup\(\);/);
  assert.match(render, /#accountAuthRepairConnection/);
  assert.match(render, /repairRemoteConnection\(\)/);
  assert.match(render, /initializeUserAuth\(\)/);
  assert.match(render, /state === "authenticated"/);
  assert.match(render, /state === "required"/);
  assert.match(render, /renderUserAuthGate\(host, onAuthenticated\);/);
  assert.match(render, /\[data-auth-mode\]/);
  assert.match(render, /button\.dataset\.authMode === "register"/);
  assert.match(render, /#accountAuthForm/);
  assert.match(render, /authMode === "register"/);
  assert.match(render, /Les mots de passe ne correspondent pas\./);
  assert.match(render, /authApi<SessionResponse>\("\/register",/);
  assert.match(render, /authApi<SessionResponse>\("\/login",/);
  assert.match(render, /#authUsername/);
  assert.match(render, /#authEmail/);
  assert.match(render, /#authIdentifier/);
  assert.match(render, /#authPassword/);
  assert.match(render, /#authPasswordConfirm/);
  assert.match(render, /currentUser = session\.user;/);
  assert.match(render, /onAuthenticated\(\);/);
  assert.match(render, /submit\?\.setAttribute\("disabled", ""\)/);
  assert.match(render, /setAttribute\("aria-busy", "true"\)/);
});

test("les initiales d'utilisateur sont stables et tombent sur U", () => {
  const initials = block(auth, "const userInitials", "const avatarMarkup");
  assert.match(initials, /user\.username\s*\.split\(\/\[\._-\]\+\/\)/);
  assert.match(initials, /\.filter\(Boolean\)/);
  assert.match(initials, /\.slice\(0, 2\)/);
  assert.match(initials, /\.map\(\(part\) => part\[0\]\?\.toUpperCase\(\) \?\? ""\)/);
  assert.match(initials, /\.join\(""\) \|\| "U"/);
  assert.ok(initials.indexOf("toUpperCase") >= 0, "majuscule");
});

test("avatarMarkup rend une image no-referrer ou un monogramme", () => {
  const avatar = block(auth, "const avatarMarkup", "export const renderUserAccountButton");
  assert.match(avatar, /user\.avatarUrl/);
  assert.match(avatar, /referrerpolicy="no-referrer"/);
  assert.match(avatar, /aria-hidden="true"/);
  assert.match(avatar, /userInitials\(user\)/);
  assert.match(avatar, /escapeAttr\(user\.avatarUrl\)/);
  assert.ok(avatar.indexOf("img") >= 0, "img");
  assert.ok(avatar.indexOf("span") >= 0, "span");
});

test("le bouton de compte expose l'utilisateur et le badge Microsoft en attente", () => {
  const button = block(auth, "export const renderUserAccountButton", "export const renderUserProfileModal");
  assert.match(button, /if \(!currentUser\) return "";/);
  assert.match(button, /microsoftPendingActionCount\(\)/);
  assert.match(button, /userProfileToggle/);
  assert.match(button, /user-profile-avatar/);
  assert.match(button, /currentUser\.username/);
  assert.match(button, /currentUser\.email/);
  assert.match(button, /user-profile-pending/);
  assert.match(button, /action Microsoft à confirmer/);
  assert.ok(button.indexOf("Gérer mon profil") >= 0, "titre");
});

test("la modale de profil liste compte, connexions, actions et suppression", () => {
  const modal = block(auth, "export const renderUserProfileModal", "export const openUserProfileModal");
  assert.match(modal, /user-profile-backdrop/);
  assert.match(modal, /role="dialog"/);
  assert.match(modal, /aria-modal="true"/);
  assert.match(modal, /data-user-profile-close/);
  assert.match(modal, /Mon compte/);
  assert.match(modal, /id="userProfileForm"/);
  assert.match(modal, /profileUsername/);
  assert.match(modal, /profileEmail/);
  assert.match(modal, /profileCurrentPassword/);
  assert.match(modal, /hasPassword/);
  assert.match(modal, /key-round/);
  assert.match(modal, /googleLinked/);
  assert.match(modal, /userLogout/);
  assert.match(modal, /Se déconnecter/);
  assert.match(modal, /deleteUserAccount/);
  assert.match(modal, /Supprimer définitivement mon compte/);
  assert.match(modal, /Suppression en cours…/);
  assert.match(modal, /renderMicrosoftConnectionSettings\(\)/);
  assert.match(modal, /renderMicrosoftPendingActions\(\)/);
  assert.ok(modal.indexOf("Votre compte Google restera lié") >= 0, "note Google");
  assert.ok(modal.indexOf("Mot de passe actuel") >= 0, "mot de passe requis");
});

test("ouvrir et fermer la modale réinitialise les états transitoires", () => {
  const open = block(auth, "export const openUserProfileModal", "export const closeUserProfileModal");
  assert.match(open, /profileOpen = true;/);
  assert.match(open, /profileError = null;/);
  assert.match(open, /profileSuccess = null;/);
  assert.match(open, /refreshMicrosoftConnection\(\);/);
  assert.match(open, /refreshMicrosoftPendingActions\(\);/);
  const close = block(auth, "export const closeUserProfileModal", "export const bindUserAccountUi");
  assert.match(close, /profileOpen = false;/);
  assert.match(close, /profileError = null;/);
  assert.match(close, /profileSuccess = null;/);
  assert.match(close, /accountDeletionError = null;/);
});

test("bindUserAccountUi câble profil, suppression et déconnexion", () => {
  const bind = block(auth, "export const bindUserAccountUi", "export const authenticatedUser");
  assert.match(bind, /#userProfileToggle/);
  assert.match(bind, /openUserProfileModal\(\);/);
  assert.match(bind, /\.user-profile-services/);
  assert.match(bind, /handleMicrosoftPendingActionClick\(event\.target/);
  assert.match(bind, /event\.preventDefault\(\);/);
  assert.match(bind, /\[data-user-profile-close\]/);
  assert.match(bind, /data-user-profile-dialog/);
  assert.match(bind, /classList\.contains\("user-profile-backdrop"\) && event\.target !== element/);
  assert.match(bind, /closeUserProfileModal\(\);/);
  assert.match(bind, /#deleteUserAccount/);
  assert.match(bind, /window\.prompt\('Pour confirmer, écrivez SUPPRIMER'\)/);
  assert.match(bind, /confirmation !== "SUPPRIMER"/);
  assert.match(bind, /authApi<void>\("\/account", \{ method: "DELETE" \}\)/);
  assert.match(bind, /window\.location\.reload\(\);/);
  assert.match(bind, /#userLogout/);
  assert.match(bind, /authApi<void>\("\/logout", \{ method: "POST" \}\)/);
  assert.match(bind, /#userProfileForm/);
  assert.match(bind, /authApi<SessionResponse>\("\/profile", \{/);
  assert.match(bind, /method: "PUT"/);
  assert.match(bind, /#profileUsername/);
  assert.match(bind, /#profileEmail/);
  assert.match(bind, /#profileCurrentPassword/);
  assert.match(bind, /Profil mis à jour\./);
  assert.ok((bind.match(/window\.location\.reload\(\)/g) ?? []).length >= 2, "rechargement après suppression et logout");
});

test("authenticatedUser expose la session courante", () => {
  assert.match(auth, /export const authenticatedUser = \(\) => currentUser;/);
});

test("l'intégration Microsoft reste couplée sans fuir hors de la modale", () => {
  assert.match(auth, /import \{[^}]*consumeMicrosoftOAuthResult/);
  assert.match(auth, /import \{[^}]*handleMicrosoftPendingActionClick/);
  assert.match(auth, /import \{[^}]*microsoftPendingActionCount/);
  assert.match(auth, /import \{[^}]*refreshMicrosoftConnection/);
  assert.match(auth, /import \{[^}]*refreshMicrosoftPendingActions/);
  assert.match(auth, /import \{[^}]*renderMicrosoftConnectionSettings/);
  assert.match(auth, /import \{[^}]*renderMicrosoftPendingActions/);
  assert.match(microsoft, /export const consumeMicrosoftOAuthResult/);
  assert.match(microsoft, /export const microsoftPendingActionCount/);
  assert.match(microsoft, /export const refreshMicrosoftConnection/);
  assert.match(microsoft, /export const refreshMicrosoftPendingActions/);
  assert.match(microsoft, /export const renderMicrosoftConnectionSettings/);
  assert.match(microsoft, /export const renderMicrosoftPendingActions/);
  assert.ok(main.indexOf("bindUserAccountUi") >= 0, "UI câblée dans main.ts");
  assert.ok(main.indexOf("initializeUserAuth") >= 0, "init câblée dans main.ts");
});
