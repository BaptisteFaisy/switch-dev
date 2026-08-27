import { remoteBaseUrl, repairRemoteConnection } from "./platform";
import {
  consumeMicrosoftOAuthResult,
  handleMicrosoftPendingActionClick,
  microsoftPendingActionCount,
  refreshMicrosoftConnection,
  refreshMicrosoftPendingActions,
  renderMicrosoftConnectionSettings,
  renderMicrosoftPendingActions,
} from "./microsoft";

export type AuthUser = {
  id: string;
  username: string;
  email: string;
  avatarUrl?: string | null;
  hasPassword: boolean;
  googleLinked: boolean;
  createdAt: number;
  updatedAt: number;
};

type AuthConfig = {
  enabled: boolean;
  registrationEnabled: boolean;
  googleEnabled: boolean;
  googleLoginUrl?: string | null;
};

type SessionResponse = { user: AuthUser };

export type AuthBootstrapState = "authenticated" | "required" | "unsupported";

class AuthApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

let authConfig: AuthConfig | null = null;
let currentUser: AuthUser | null = null;
let authMode: "login" | "register" = "login";
let authGateError: string | null = null;
let profileOpen = false;
let profileError: string | null = null;
let profileSuccess: string | null = null;
let accountDeletionError: string | null = null;
let accountDeletionBusy = false;

const isRepairableConnectionError = (error: string | null) =>
  Boolean(error && /failed to fetch|networkerror|network request failed|fetch failed|load failed/i.test(error));

const escapeHtml = (value: string) =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");

const escapeAttr = escapeHtml;

const remoteHostLabel = () => {
  try {
    return new URL(remoteBaseUrl()).host;
  } catch {
    return remoteBaseUrl();
  }
};

const googleLoginUrl = () => {
  const configured = authConfig?.googleLoginUrl?.trim();
  if (configured) {
    try {
      const url = new URL(configured);
      if (url.protocol === "https:" || url.protocol === "http:") return url.href;
    } catch {
      // Une ancienne version du serveur peut ne pas fournir une URL exploitable.
    }
  }
  return `${remoteBaseUrl()}/api/auth/google/start`;
};

const authApi = async <T>(
  path: string,
  options: { method?: string; body?: unknown } = {},
): Promise<T> => {
  const response = await fetch(`${remoteBaseUrl()}/api/auth${path}`, {
    method: options.method ?? "GET",
    credentials: "include",
    headers: options.body === undefined ? {} : { "Content-Type": "application/json" },
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  const text = await response.text();
  let value: any = null;
  if (text) {
    try {
      value = JSON.parse(text);
    } catch {
      value = null;
    }
  }
  if (!response.ok) {
    throw new AuthApiError(
      value?.error?.message || value?.message || response.statusText || "Erreur d'authentification",
      response.status,
    );
  }
  return value as T;
};

const consumeOAuthResult = () => {
  // Le retour de liaison Microsoft arrive sur la meme page que celui de Google
  // et n'a de sens qu'en mode distant : il se consomme donc au meme endroit,
  // sinon les parametres resteraient dans l'URL apres un rechargement.
  consumeMicrosoftOAuthResult();
  const url = new URL(window.location.href);
  const oauthError = url.searchParams.get("auth_error");
  if (oauthError) authGateError = oauthError;
  if (!oauthError && url.searchParams.get("auth") === "google") {
    profileSuccess = "Connexion Google réussie.";
  }
  if (url.searchParams.has("auth") || url.searchParams.has("auth_error")) {
    url.searchParams.delete("auth");
    url.searchParams.delete("auth_error");
    window.history.replaceState({}, "", `${url.pathname}${url.search}${url.hash}`);
  }
};

export const initializeUserAuth = async (): Promise<AuthBootstrapState> => {
  consumeOAuthResult();
  try {
    authConfig = await authApi<AuthConfig>("/config");
  } catch (error) {
    if (error instanceof AuthApiError && error.status === 404) return "unsupported";
    throw error;
  }
  // Une origine Tauri locale ou un ancien proxy peut repondre 200 avec un
  // corps JSON `null`. Ce n'est pas une configuration d'authentification et
  // cela ne doit jamais faire planter l'ecran de connexion sur `.enabled`.
  if (!authConfig || typeof authConfig.enabled !== "boolean" || !authConfig.enabled) {
    return "unsupported";
  }

  try {
    const session = await authApi<SessionResponse>("/me");
    currentUser = session.user;
    authGateError = null;
    return "authenticated";
  } catch (error) {
    if (error instanceof AuthApiError && error.status === 401) {
      currentUser = null;
      return "required";
    }
    throw error;
  }
};

const googleMark = () => `
  <svg class="auth-google-mark" viewBox="0 0 24 24" aria-hidden="true">
    <path fill="#4285F4" d="M21.6 12.23c0-.71-.06-1.4-.18-2.07H12v3.91h5.38a4.6 4.6 0 0 1-2 3.02v2.54h3.24c1.9-1.75 2.98-4.33 2.98-7.4Z"/>
    <path fill="#34A853" d="M12 22c2.7 0 4.98-.9 6.63-2.37l-3.24-2.54c-.9.6-2.05.96-3.39.96-2.61 0-4.82-1.77-5.61-4.14H3.04v2.62A10 10 0 0 0 12 22Z"/>
    <path fill="#FBBC05" d="M6.39 13.91A6 6 0 0 1 6.07 12c0-.66.11-1.3.32-1.91V7.47H3.04A10 10 0 0 0 2 12c0 1.63.39 3.17 1.04 4.53l3.35-2.62Z"/>
    <path fill="#EA4335" d="M12 5.95c1.47 0 2.79.5 3.83 1.5l2.87-2.87A9.63 9.63 0 0 0 12 2a10 10 0 0 0-8.96 5.47l3.35 2.62C7.18 7.72 9.39 5.95 12 5.95Z"/>
  </svg>`;

const authGateMarkup = () => {
  const registrationEnabled = authConfig?.registrationEnabled ?? false;
  if (!registrationEnabled && authMode === "register") authMode = "login";
  const isRegister = authMode === "register";
  return `
    <main class="account-auth" aria-labelledby="accountAuthTitle">
      <section class="account-auth-panel">
        <div class="account-auth-brand" aria-hidden="true"><span>S</span></div>
        <header class="account-auth-head">
          <span class="account-auth-eyebrow">Codex Switch Terminal</span>
          <h1 id="accountAuthTitle">${isRegister ? "Créer votre compte" : "Content de vous revoir"}</h1>
          <p>${isRegister
            ? "Créez vos identifiants pour accéder à votre espace de travail."
            : "Connectez-vous pour retrouver votre espace de travail."}</p>
        </header>

        ${registrationEnabled ? `<div class="account-auth-tabs" role="tablist" aria-label="Type d'authentification">
          <button type="button" role="tab" data-auth-mode="login" aria-selected="${!isRegister}" class="${!isRegister ? "active" : ""}">Connexion</button>
          <button type="button" role="tab" data-auth-mode="register" aria-selected="${isRegister}" class="${isRegister ? "active" : ""}">Créer un compte</button>
        </div>` : ""}

        ${authGateError ? `<div class="account-auth-message error" role="alert">${escapeHtml(authGateError)}</div>` : ""}
        ${isRepairableConnectionError(authGateError) ? `<div class="account-auth-repair">
          <strong>La connexion au serveur a ete interrompue.</strong>
          <p>Remettez automatiquement l'API sur cette adresse et nettoyez le cache obsolete.</p>
          <button id="accountAuthRepairConnection" type="button">Reparer la connexion</button>
          <small id="accountAuthRepairStatus" role="status" aria-live="polite"></small>
        </div>` : ""}

        <form id="accountAuthForm" class="account-auth-form" data-mode="${authMode}">
          ${isRegister ? `<label>
            <span>Nom d'utilisateur</span>
            <input id="authUsername" name="username" autocomplete="username" minlength="3" maxlength="32" pattern="[A-Za-z0-9_.-]+" placeholder="jean.dupont" required autofocus />
            <small>Lettres, chiffres, point, tiret ou underscore.</small>
          </label>
          <label>
            <span>Adresse e-mail</span>
            <input id="authEmail" name="email" type="email" autocomplete="email" placeholder="vous@exemple.fr" required />
          </label>` : `<label>
            <span>E-mail ou nom d'utilisateur</span>
            <input id="authIdentifier" name="identifier" autocomplete="username" placeholder="vous@exemple.fr" required autofocus />
          </label>`}
          <label>
            <span>Mot de passe</span>
            <input id="authPassword" name="password" type="password" autocomplete="${isRegister ? "new-password" : "current-password"}" minlength="${isRegister ? "10" : "1"}" placeholder="${isRegister ? "10 caractères minimum" : "Votre mot de passe"}" required />
          </label>
          ${isRegister ? `<label>
            <span>Confirmer le mot de passe</span>
            <input id="authPasswordConfirm" name="passwordConfirm" type="password" autocomplete="new-password" minlength="10" placeholder="Répétez votre mot de passe" required />
          </label>` : ""}
          <button class="account-auth-submit" type="submit">
            <span>${isRegister ? "Créer mon compte" : "Se connecter"}</span>
            <span aria-hidden="true">→</span>
          </button>
        </form>

        ${authConfig?.googleEnabled ? `<div class="account-auth-separator"><span>ou</span></div>
          <a class="account-google-button" href="${escapeAttr(googleLoginUrl())}">
            ${googleMark()}<span>Continuer avec Google</span>
          </a>` : ""}

        <footer class="account-auth-footer">
          <span>Connexion sécurisée</span>
          <span aria-hidden="true">•</span>
          <span>${escapeHtml(remoteHostLabel())}</span>
        </footer>
      </section>
    </main>`;
};

export const renderUserAuthGate = (
  host: HTMLElement,
  onAuthenticated: () => void | Promise<void>,
) => {
  host.innerHTML = authGateMarkup();
  host.querySelector<HTMLButtonElement>("#accountAuthRepairConnection")?.addEventListener("click", async (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    const status = host.querySelector<HTMLElement>("#accountAuthRepairStatus");
    button.disabled = true;
    button.textContent = "Reparation en cours...";
    if (status) status.textContent = "Verification du serveur et nettoyage du cache...";
    try {
      await repairRemoteConnection();
      authGateError = null;
      const state = await initializeUserAuth();
      if (state === "authenticated") {
        await onAuthenticated();
        return;
      }
      if (state === "required") {
        renderUserAuthGate(host, onAuthenticated);
        return;
      }
      authGateError = "Le serveur ne propose pas la connexion utilisateur attendue.";
    } catch (error) {
      authGateError = error instanceof Error ? error.message : String(error);
    }
    renderUserAuthGate(host, onAuthenticated);
  });
  host.querySelectorAll<HTMLButtonElement>("[data-auth-mode]").forEach((button) => {
    button.addEventListener("click", () => {
      authMode = button.dataset.authMode === "register" ? "register" : "login";
      authGateError = null;
      renderUserAuthGate(host, onAuthenticated);
    });
  });

  host.querySelector<HTMLFormElement>("#accountAuthForm")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    const submit = form.querySelector<HTMLButtonElement>("button[type='submit']");
    submit?.setAttribute("disabled", "");
    form.setAttribute("aria-busy", "true");
    authGateError = null;
    try {
      let session: SessionResponse;
      if (authMode === "register") {
        const password = form.querySelector<HTMLInputElement>("#authPassword")?.value ?? "";
        const confirmation = form.querySelector<HTMLInputElement>("#authPasswordConfirm")?.value ?? "";
        if (password !== confirmation) throw new Error("Les mots de passe ne correspondent pas.");
        session = await authApi<SessionResponse>("/register", {
          method: "POST",
          body: {
            username: form.querySelector<HTMLInputElement>("#authUsername")?.value ?? "",
            email: form.querySelector<HTMLInputElement>("#authEmail")?.value ?? "",
            password,
          },
        });
      } else {
        session = await authApi<SessionResponse>("/login", {
          method: "POST",
          body: {
            identifier: form.querySelector<HTMLInputElement>("#authIdentifier")?.value ?? "",
            password: form.querySelector<HTMLInputElement>("#authPassword")?.value ?? "",
          },
        });
      }
      currentUser = session.user;
      await onAuthenticated();
    } catch (error) {
      authGateError = error instanceof Error ? error.message : String(error);
      renderUserAuthGate(host, onAuthenticated);
    }
  });
};

const userInitials = (user: AuthUser) =>
  user.username
    .split(/[._-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? "")
    .join("") || "U";

const avatarMarkup = (user: AuthUser, className: string) =>
  user.avatarUrl
    ? `<img class="${className}" src="${escapeAttr(user.avatarUrl)}" alt="" referrerpolicy="no-referrer" />`
    : `<span class="${className}" aria-hidden="true">${escapeHtml(userInitials(user))}</span>`;

export const renderUserAccountButton = () => {
  if (!currentUser) return "";
  const pending = microsoftPendingActionCount();
  // Un agent autonome depose ses brouillons sans que personne ne regarde. Le
  // compteur sur le bouton de compte est le seul endroit visible depuis
  // n'importe quelle vue : sans lui, un e-mail prepare expirerait en silence.
  const badge = pending
    ? `<span class="user-profile-pending" title="${pending} action Microsoft à confirmer" aria-label="${pending} action Microsoft à confirmer">${pending}</span>`
    : "";
  return `<button type="button" id="userProfileToggle" class="user-profile-toggle" title="Gérer mon profil">
    ${avatarMarkup(currentUser, "user-profile-avatar")}
    <span><strong>${escapeHtml(currentUser.username)}</strong><small>${escapeHtml(currentUser.email)}</small></span>
    ${badge}
  </button>`;
};

export const renderUserProfileModal = () => {
  if (!currentUser || !profileOpen) return "";
  return `<div class="modal-backdrop user-profile-backdrop" data-user-profile-close>
    <section class="modal user-profile-modal" role="dialog" aria-modal="true" aria-labelledby="userProfileTitle" tabindex="-1" data-user-profile-dialog>
      <header class="user-profile-head">
        <div class="user-profile-identity">
          ${avatarMarkup(currentUser, "user-profile-avatar large")}
          <div><span>Mon compte</span><h2 id="userProfileTitle">${escapeHtml(currentUser.username)}</h2><p>${escapeHtml(currentUser.email)}</p></div>
        </div>
        <button type="button" class="icon-button" data-user-profile-close aria-label="Fermer"><i data-lucide="x"></i></button>
      </header>
      <form id="userProfileForm" class="user-profile-form">
        ${profileError ? `<div class="account-auth-message error" role="alert">${escapeHtml(profileError)}</div>` : ""}
        ${profileSuccess ? `<div class="account-auth-message success" role="status">${escapeHtml(profileSuccess)}</div>` : ""}
        <label>
          <span>Nom d'utilisateur</span>
          <input id="profileUsername" autocomplete="username" minlength="3" maxlength="32" pattern="[A-Za-z0-9_.-]+" value="${escapeAttr(currentUser.username)}" required />
          <small>Vous pouvez le modifier à tout moment.</small>
        </label>
        <label>
          <span>Adresse e-mail</span>
          <input id="profileEmail" type="email" autocomplete="email" value="${escapeAttr(currentUser.email)}" required />
          <small>${currentUser.googleLinked ? "Votre compte Google restera lié même si cette adresse change." : "Votre mot de passe sera demandé si cette adresse change."}</small>
        </label>
        ${currentUser.hasPassword ? `<label>
          <span>Mot de passe actuel <em>requis pour changer l'e-mail</em></span>
          <input id="profileCurrentPassword" type="password" autocomplete="current-password" placeholder="Laisser vide si l'e-mail ne change pas" />
        </label>` : ""}
        <div class="user-profile-connections" aria-label="Méthodes de connexion">
          <span>Méthodes de connexion</span>
          <div>
            ${currentUser.hasPassword ? `<span class="user-profile-provider"><i data-lucide="key-round"></i> Mot de passe</span>` : ""}
            ${currentUser.googleLinked ? `<span class="user-profile-provider google">${googleMark()} Google</span>` : ""}
          </div>
        </div>
        <div class="user-profile-actions">
          <button type="button" id="userLogout" class="tool-button danger"><i data-lucide="log-out"></i><span>Se déconnecter</span></button>
          <span></span>
          <button type="button" class="tool-button" data-user-profile-close>Annuler</button>
          <button type="submit" class="tool-button primary"><i data-lucide="save"></i><span>Enregistrer</span></button>
        </div>
        <section class="account-deletion-panel" aria-labelledby="accountDeletionTitle">
          <h3 id="accountDeletionTitle">Supprimer mon compte</h3>
          <p>Cette action supprime définitivement votre compte Duello, votre progression, vos copies, vos résultats, votre profil public et vos appareils associés. Les obligations légales éventuellement nécessaires sont conservées uniquement lorsque la loi l’impose.</p>
          ${accountDeletionError ? `<div class="account-auth-message error" role="alert">${escapeHtml(accountDeletionError)}</div>` : ""}
          <button type="button" id="deleteUserAccount" class="tool-button danger" ${accountDeletionBusy ? "disabled" : ""}><i data-lucide="trash-2"></i><span>${accountDeletionBusy ? "Suppression en cours…" : "Supprimer définitivement mon compte"}</span></button>
        </section>
      </form>
      <div class="user-profile-services">
        ${renderMicrosoftConnectionSettings()}
        ${renderMicrosoftPendingActions()}
      </div>
    </section>
  </div>`;
};

export const openUserProfileModal = () => {
  profileOpen = true;
  profileError = null;
  profileSuccess = null;
  void refreshMicrosoftConnection();
  void refreshMicrosoftPendingActions();
};

export const closeUserProfileModal = () => {
  profileOpen = false;
  profileError = null;
  profileSuccess = null;
  accountDeletionError = null;
};

export const bindUserAccountUi = (rerender: () => void) => {
  document.querySelector<HTMLButtonElement>("#userProfileToggle")?.addEventListener("click", () => {
    openUserProfileModal();
    rerender();
  });

  // Les cartes de confirmation vivent aussi dans cette modale : sans relais ici,
  // les boutons Envoyer et Annuler seraient inertes hors d'une conversation.
  document
    .querySelector<HTMLElement>(".user-profile-services")
    ?.addEventListener("click", (event) => {
      if (handleMicrosoftPendingActionClick(event.target as HTMLElement | null)) {
        event.preventDefault();
      }
    });

  document.querySelectorAll<HTMLElement>("[data-user-profile-close]").forEach((element) => {
    element.addEventListener("click", (event) => {
      if (
        element.hasAttribute("data-user-profile-dialog") ||
        (element.classList.contains("user-profile-backdrop") && event.target !== element)
      ) return;
      closeUserProfileModal();
      rerender();
    });
  });

  document.querySelector<HTMLButtonElement>("#deleteUserAccount")?.addEventListener("click", async () => {
    if (!currentUser || accountDeletionBusy) return;
    const confirmation = window.prompt('Pour confirmer, écrivez SUPPRIMER');
    if (confirmation !== "SUPPRIMER") return;
    accountDeletionBusy = true;
    accountDeletionError = null;
    rerender();
    try {
      await authApi<void>("/account", { method: "DELETE" });
      currentUser = null;
      window.location.reload();
    } catch (error) {
      accountDeletionError = error instanceof Error ? error.message : String(error);
      accountDeletionBusy = false;
      rerender();
    }
  });

  document.querySelector<HTMLButtonElement>("#userLogout")?.addEventListener("click", async () => {
    try {
      await authApi<void>("/logout", { method: "POST" });
    } finally {
      currentUser = null;
      window.location.reload();
    }
  });

  document.querySelector<HTMLFormElement>("#userProfileForm")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!currentUser) return;
    const form = event.currentTarget as HTMLFormElement;
    const submit = form.querySelector<HTMLButtonElement>("button[type='submit']");
    submit?.setAttribute("disabled", "");
    form.setAttribute("aria-busy", "true");
    profileError = null;
    profileSuccess = null;
    try {
      const response = await authApi<SessionResponse>("/profile", {
        method: "PUT",
        body: {
          username: form.querySelector<HTMLInputElement>("#profileUsername")?.value ?? "",
          email: form.querySelector<HTMLInputElement>("#profileEmail")?.value ?? "",
          currentPassword: form.querySelector<HTMLInputElement>("#profileCurrentPassword")?.value || null,
        },
      });
      currentUser = response.user;
      profileSuccess = "Profil mis à jour.";
    } catch (error) {
      profileError = error instanceof Error ? error.message : String(error);
    }
    rerender();
  });
};

export const authenticatedUser = () => currentUser;
