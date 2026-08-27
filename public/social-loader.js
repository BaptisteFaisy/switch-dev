(() => {
  if (window.__switchSocialAnalyticsLoader) return;
  window.__switchSocialAnalyticsLoader = true;

  const entrySelector = "[data-switch-social-entry]";
  let available = false;
  let probePromise = null;
  let injectionTimer = 0;
  let lastTrigger = null;
  let initialFrameUrl = "/social/";

  const callbackQuery = new URLSearchParams(window.location.search);
  const callbackStatus = callbackQuery.get("switch_social");
  const callbackProvider = callbackQuery.get("provider") || "account";
  if (callbackStatus === "connected") {
    initialFrameUrl = `/social/?connected=${encodeURIComponent(callbackProvider)}`;
  } else if (callbackStatus === "error") {
    initialFrameUrl = `/social/?connect_error=${encodeURIComponent(callbackProvider)}`;
  }
  if (callbackStatus) {
    callbackQuery.delete("switch_social");
    callbackQuery.delete("provider");
    const cleaned = callbackQuery.toString();
    history.replaceState(
      history.state ?? {},
      "",
      `${window.location.pathname}${cleaned ? `?${cleaned}` : ""}${window.location.hash}`,
    );
  }

  const style = document.createElement("style");
  style.textContent = `
    .switch-social-nav-mark{display:grid;place-items:center;flex:0 0 auto;width:28px;height:28px;border:1px solid color-mix(in srgb,#65e8df 30%,transparent);border-radius:8px;color:#65e8df;background:color-mix(in srgb,#65e8df 7%,transparent);font:900 8px/1 ui-sans-serif,system-ui,sans-serif;letter-spacing:.04em}
    .switch-social-toolbar-entry{gap:7px!important}.switch-social-toolbar-entry .switch-social-nav-mark{width:20px;height:20px;border-radius:6px;font-size:7px}
    .switch-social-sheet-entry{position:relative}.switch-social-sheet-entry::after{content:'DEV';position:absolute;right:8px;top:7px;padding:2px 4px;border:1px solid rgba(243,198,110,.3);border-radius:4px;color:#f3c66e;font-size:6px;font-weight:900;letter-spacing:.06em}
    #switchSocialDialog{width:calc(100vw - 24px);height:calc(100dvh - 24px);max-width:none;max-height:none;margin:auto;padding:0;border:1px solid #2d3533;border-radius:14px;color:#f2f5f3;background:#070909;box-shadow:0 28px 90px rgba(0,0,0,.64);overflow:hidden}
    #switchSocialDialog::backdrop{background:rgba(0,0,0,.72);backdrop-filter:blur(5px)}
    .switch-social-shell{display:grid;width:100%;height:100%;grid-template-rows:54px minmax(0,1fr)}
    .switch-social-shell-head{display:flex;align-items:center;gap:12px;padding:0 max(14px,env(safe-area-inset-left));border-bottom:1px solid #242a29;background:rgba(8,10,10,.97)}
    .switch-social-shell-action{display:inline-flex;min-height:34px;align-items:center;gap:7px;padding:0 10px;border:1px solid #303736;border-radius:9px;color:#c3ccca;background:#111514;cursor:pointer;font:700 10px/1 ui-sans-serif,system-ui,sans-serif;text-decoration:none}
    .switch-social-shell-action:hover{color:#fff;border-color:#6a7672}.switch-social-shell-title{display:grid;gap:2px;min-width:0}.switch-social-shell-title strong{font-size:11px}.switch-social-shell-title small{color:#7e8985;font-size:8px;letter-spacing:.1em;text-transform:uppercase}
    .switch-social-shell-tools{display:flex;align-items:center;gap:8px;margin-left:auto}.switch-social-shell-badge{padding:5px 7px;border:1px solid rgba(243,198,110,.26);border-radius:6px;color:#f3c66e;font-size:7px;font-weight:900;letter-spacing:.07em}
    #switchSocialFrame{display:block;width:100%;height:100%;border:0;background:#070909}
    ${entrySelector}.active .switch-social-nav-mark,${entrySelector}[aria-current="page"] .switch-social-nav-mark{color:#07110b;border-color:#eaf7ef;background:#eaf7ef}
    @media(max-width:860px){#switchSocialDialog{width:100vw;height:100dvh;border:0;border-radius:0}.switch-social-shell{grid-template-rows:calc(50px + env(safe-area-inset-top)) minmax(0,1fr)}.switch-social-shell-head{padding-top:env(safe-area-inset-top)}.switch-social-shell-badge,.switch-social-open-page{display:none}}
    @media(prefers-reduced-motion:reduce){#switchSocialDialog::backdrop{backdrop-filter:none}}
  `;
  document.head.appendChild(style);

  function dialogElement() {
    let dialog = document.querySelector("#switchSocialDialog");
    if (dialog) return dialog;
    dialog = document.createElement("dialog");
    dialog.id = "switchSocialDialog";
    dialog.setAttribute("aria-label", "Dashboard des réseaux sociaux");
    dialog.innerHTML = `
      <section class="switch-social-shell">
        <header class="switch-social-shell-head">
          <button class="switch-social-shell-action" type="button" data-switch-social-close><span aria-hidden="true">←</span> Revenir à Switch</button>
          <span class="switch-social-shell-title"><strong>Réseaux sociaux</strong><small>Instagram &amp; TikTok</small></span>
          <span class="switch-social-shell-tools">
            <span class="switch-social-shell-badge">DÉVELOPPEMENT</span>
            <a class="switch-social-shell-action switch-social-open-page" href="/social/" target="_blank" rel="noopener">Ouvrir dans une page</a>
          </span>
        </header>
        <div data-switch-social-frame-host></div>
      </section>`;
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      closeSocial();
    });
    dialog.addEventListener("close", () => {
      dialog.querySelector("#switchSocialFrame")?.remove();
      syncActiveEntries();
      lastTrigger?.focus?.({ preventScroll: true });
    });
    document.body.appendChild(dialog);
    return dialog;
  }

  function syncActiveEntries() {
    const opened = document.querySelector("#switchSocialDialog")?.open === true;
    document.querySelectorAll(entrySelector).forEach((entry) => {
      entry.classList.toggle("active", opened);
      if (opened) entry.setAttribute("aria-current", "page");
      else entry.removeAttribute("aria-current");
    });
  }

  function openSocial(trigger = null) {
    if (!available) return;
    const dialog = dialogElement();
    const frameHost = dialog.querySelector("[data-switch-social-frame-host]");
    if (!frameHost.querySelector("iframe")) {
      const frame = document.createElement("iframe");
      frame.id = "switchSocialFrame";
      frame.title = "Réseaux sociaux — Instagram et TikTok";
      frame.src = initialFrameUrl;
      frame.sandbox = "allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox allow-top-navigation-by-user-activation";
      frameHost.appendChild(frame);
      initialFrameUrl = "/social/";
    }
    lastTrigger = trigger instanceof HTMLElement ? trigger : document.activeElement;
    if (!dialog.open) dialog.showModal();
    syncActiveEntries();
    if (window.location.hash !== "#switch-social") {
      history.pushState({ ...(history.state ?? {}), switchSocial: true }, "", "#switch-social");
    }
    dialog.querySelector("[data-switch-social-close]")?.focus({ preventScroll: true });
  }

  function hideSocial() {
    const dialog = document.querySelector("#switchSocialDialog");
    if (dialog?.open) dialog.close();
  }

  function closeSocial() {
    if (window.location.hash === "#switch-social" && history.state?.switchSocial) {
      history.back();
      return;
    }
    hideSocial();
    if (window.location.hash === "#switch-social") {
      history.replaceState(history.state ?? {}, "", `${window.location.pathname}${window.location.search}`);
    }
  }

  function sideEntry() {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.switchSocialEntry = "side";
    button.title = "Vues Instagram et TikTok par jour";
    button.innerHTML = `<span class="chat-context-icon switch-social-nav-mark">S+</span><span class="chat-context-copy"><strong>Réseaux sociaux</strong><small>Vues quotidiennes par compte</small></span>`;
    return button;
  }

  function toolbarEntry() {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "tool-button switch-social-toolbar-entry";
    button.dataset.switchSocialEntry = "toolbar";
    button.title = "Réseaux sociaux · vues quotidiennes";
    button.innerHTML = `<span class="switch-social-nav-mark">S+</span><span>Social</span>`;
    return button;
  }

  function sheetEntry() {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "switch-social-sheet-entry";
    button.dataset.switchSocialEntry = "sheet";
    button.setAttribute("role", "menuitem");
    button.innerHTML = `<span class="switch-social-nav-mark">S+</span><span>Réseaux sociaux</span>`;
    return button;
  }

  function injectEntries() {
    if (!available) return;
    const sideDashboard = document.querySelector(".chat-side-tools #dashboardToggle");
    if (sideDashboard && !document.querySelector('[data-switch-social-entry="side"]')) {
      sideDashboard.insertAdjacentElement("afterend", sideEntry());
    }
    document.querySelectorAll("#dashboardToggle.tool-button").forEach((dashboardButton) => {
      if (!dashboardButton.parentElement?.querySelector('[data-switch-social-entry="toolbar"]')) {
        dashboardButton.insertAdjacentElement("afterend", toolbarEntry());
      }
    });
    const mobileGrid = document.querySelector(".m-sheet-grid");
    if (mobileGrid && !mobileGrid.querySelector('[data-switch-social-entry="sheet"]')) {
      const dashboardEntry = mobileGrid.querySelector('[data-view="dashboard"]');
      if (dashboardEntry) dashboardEntry.insertAdjacentElement("afterend", sheetEntry());
      else mobileGrid.appendChild(sheetEntry());
    }
    syncActiveEntries();
  }

  function scheduleInjection() {
    if (!available || injectionTimer) return;
    injectionTimer = window.setTimeout(() => {
      injectionTimer = 0;
      injectEntries();
    }, 120);
  }

  async function probeAvailability() {
    if (probePromise) return probePromise;
    probePromise = (async () => {
      const controller = new AbortController();
      const timeout = window.setTimeout(() => controller.abort(), 2_500);
      try {
        const response = await fetch("/api/social/availability", {
          credentials: "same-origin",
          cache: "no-store",
          headers: { accept: "application/json" },
          signal: controller.signal,
        });
        const payload = response.ok ? await response.json() : null;
        available = payload?.available === true;
      } catch {
        available = false;
      } finally {
        window.clearTimeout(timeout);
      }
      if (available) {
        scheduleInjection();
        if (window.location.hash === "#switch-social") openSocial();
      } else {
        document.querySelectorAll(entrySelector).forEach((entry) => entry.remove());
        hideSocial();
      }
      return available;
    })().finally(() => { probePromise = null; });
    return probePromise;
  }

  document.addEventListener("click", (event) => {
    const entry = event.target.closest?.(entrySelector);
    if (entry) {
      event.preventDefault();
      openSocial(entry);
      return;
    }
    if (event.target.closest?.("[data-switch-social-close]")) closeSocial();
  });
  window.addEventListener("popstate", () => {
    if (window.location.hash === "#switch-social") openSocial();
    else hideSocial();
  });
  window.addEventListener("message", (event) => {
    const frameWindow = document.querySelector("#switchSocialFrame")?.contentWindow;
    if (
      event.origin === window.location.origin
      && event.source === frameWindow
      && event.data?.type === "switch-social-close"
    ) closeSocial();
  });
  window.addEventListener("online", () => void probeAvailability());
  window.addEventListener("focus", () => {
    if (!available) void probeAvailability();
  });

  const observer = new MutationObserver(scheduleInjection);
  const observe = () => {
    const app = document.querySelector("#app");
    if (app) observer.observe(app, { childList: true, subtree: true });
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", observe, { once: true });
  } else observe();
  void probeAvailability();
})();
