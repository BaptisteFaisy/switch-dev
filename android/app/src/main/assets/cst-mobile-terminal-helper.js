/*
 * Codex Terminal — assistant mobile des terminaux xterm.
 *
 * Injecté par MainActivity (cst-mobile-terminal-helper.js) à la fin du
 * chargement de chaque page. Il permet de coder dans les terminaux depuis un
 * téléphone, là où un clavier physique n'existe pas :
 *
 *  - une barre compacte sur une seule ligne (Échap, Tab, Ctrl/Alt one-shot,
 *    flèches, Ctrl+V et Ctrl+C) ;
 *  - la focalisation du champ texte caché de xterm et l'ouverture du clavier
 *    virtuel au toucher du terminal ;
 *  - l'envoi direct des séquences terminal via l'événement d'entrée de xterm.
 *    Un KeyboardEvent synthétique reste disponible uniquement en repli.
 *
 * Le script est idempotent (un seul install par page) et ne s'active que sur
 * les écrans tactiles. La barre n'apparaît que lorsqu'un terminal est visible
 * et qu'aucune modale n'est ouverte.
 */
(function () {
  "use strict";

  if (window.__cstMobileTerminalHelper) return;
  window.__cstMobileTerminalHelper = true;

  var isTouchDevice =
    "ontouchstart" in window || (navigator.maxTouchPoints && navigator.maxTouchPoints > 0);
  if (!isTouchDevice) return;

  var MODIFIER_DISARM_TIMEOUT_MS = 8000;

  var state = {
    ctrl: false,
    alt: false,
    ctrlTimer: 0,
    altTimer: 0,
    toolbar: null
  };

  function bridge() {
    return window.CstAndroid || null;
  }

  function visibleTerminalHost() {
    var hosts = document.querySelectorAll("[data-terminal-host]");
    for (var i = 0; i < hosts.length; i++) {
      var rect = hosts[i].getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) return hosts[i];
    }
    return null;
  }

  function visibleModal() {
    var modals = document.querySelectorAll('[aria-modal="true"]');
    for (var i = 0; i < modals.length; i++) {
      var rect = modals[i].getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) return true;
    }
    return false;
  }

  function terminalTarget() {
    var active = document.activeElement;
    if (
      active &&
      active.classList &&
      active.classList.contains("xterm-helper-textarea")
    ) {
      var activeHost = active.closest ? active.closest("[data-terminal-host]") : null;
      if (activeHost) return { host: activeHost, textarea: active };
    }
    var host = visibleTerminalHost();
    if (host) {
      var inside = host.querySelector(".xterm-helper-textarea");
      if (inside) return { host: host, textarea: inside };
    }
    var fallback = document.querySelector(".xterm-helper-textarea");
    var fallbackHost = fallback && fallback.closest
      ? fallback.closest("[data-terminal-host]")
      : null;
    return fallback && fallbackHost
      ? { host: fallbackHost, textarea: fallback }
      : null;
  }

  function focusTextarea(textarea) {
    if (!textarea) return;
    try {
      textarea.focus({ preventScroll: true });
    } catch (ignored) {
      textarea.focus();
    }
  }

  function requestKeyboard(show) {
    var nativeBridge = bridge();
    if (!nativeBridge) return;
    try {
      if (show && typeof nativeBridge.showKeyboard === "function") {
        nativeBridge.showKeyboard();
      } else if (!show && typeof nativeBridge.hideKeyboard === "function") {
        nativeBridge.hideKeyboard();
      }
    } catch (ignored) {
      // Le pont natif peut manquer sur les premières versions ; sans lui,
      // le clavier virtuel s'ouvre quand même via la focalisation.
    }
  }

  function terminalData(options) {
    var key = options.key;
    var ctrl = !!options.ctrl;
    var alt = !!options.alt;
    var shift = !!options.shift;
    var arrowFinal = {
      ArrowUp: "A",
      ArrowDown: "B",
      ArrowRight: "C",
      ArrowLeft: "D"
    }[key];
    var data = null;

    if (arrowFinal) {
      var modifier = 1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0);
      return modifier === 1
        ? "\u001b[" + arrowFinal
        : "\u001b[1;" + modifier + arrowFinal;
    }

    if (key === "Escape") data = "\u001b";
    else if (key === "Tab") data = shift ? "\u001b[Z" : "\t";
    else if (key === "Backspace") data = ctrl ? "\b" : "\u007f";
    else if (key === "Enter") data = "\r";
    else if (typeof key === "string" && key.length === 1) {
      data = key;
      if (ctrl) {
        var upper = key.toUpperCase().charCodeAt(0);
        if (upper >= 64 && upper <= 95) data = String.fromCharCode(upper - 64);
      }
    }

    if (data === null) return null;
    return alt ? "\u001b" + data : data;
  }

  function dispatchXtermInput(textarea, data) {
    if (!textarea || !data || typeof window.InputEvent !== "function") return false;
    try {
      var event = new InputEvent("input", {
        bubbles: true,
        cancelable: true,
        composed: false,
        data: data,
        inputType: "insertText"
      });
      // xterm émet exactement la séquence demandée sur onData, sans produire
      // de keydown global. Certaines versions traitent l'événement sans
      // l'annuler : une construction et une distribution réussies suffisent.
      textarea.dispatchEvent(event);
      return true;
    } catch (ignored) {
      return false;
    }
  }

  function dispatchKey(options) {
    var target = terminalTarget();
    if (!target) return false;
    focusTextarea(target.textarea);
    var data = terminalData(options);
    if (data && dispatchXtermInput(target.textarea, data)) return true;

    // Repli pour un WebView trop ancien pour construire InputEvent.
    var init = {
      key: options.key,
      code: options.code || "",
      bubbles: true,
      cancelable: true,
      composed: true
    };
    if (options.ctrl) init.ctrlKey = true;
    if (options.alt) init.altKey = true;
    if (options.shift) init.shiftKey = true;
    try {
      target.textarea.dispatchEvent(new KeyboardEvent("keydown", init));
      return true;
    } catch (ignored) {
      return false;
    }
  }

  function setModifierVisual(name, active) {
    if (!state.toolbar) return;
    var button = state.toolbar.querySelector('[data-cst-modifier="' + name + '"]');
    if (button) button.classList.toggle("cst-active", active);
  }

  function disarmModifiers() {
    if (state.ctrl) {
      state.ctrl = false;
      window.clearTimeout(state.ctrlTimer);
      setModifierVisual("ctrl", false);
    }
    if (state.alt) {
      state.alt = false;
      window.clearTimeout(state.altTimer);
      setModifierVisual("alt", false);
    }
  }

  function armModifier(name) {
    if (state[name]) {
      state[name] = false;
      window.clearTimeout(name === "ctrl" ? state.ctrlTimer : state.altTimer);
      setModifierVisual(name, false);
      return;
    }
    state[name] = true;
    window.clearTimeout(name === "ctrl" ? state.ctrlTimer : state.altTimer);
    var timer = window.setTimeout(function () {
      state[name] = false;
      setModifierVisual(name, false);
    }, MODIFIER_DISARM_TIMEOUT_MS);
    if (name === "ctrl") state.ctrlTimer = timer;
    else state.altTimer = timer;
    setModifierVisual(name, true);
  }

  function sendWithSticky(key, code) {
    var handled = dispatchKey({
      key: key,
      code: code || key,
      ctrl: state.ctrl,
      alt: state.alt
    });
    if (handled) disarmModifiers();
    return handled;
  }

  function sendShortcut(key, code, ctrl, alt, shift) {
    dispatchKey({
      key: key,
      code: code,
      ctrl: !!ctrl,
      alt: !!alt,
      shift: !!shift
    });
    disarmModifiers();
  }

  function pasteFromClipboard() {
    var target = terminalTarget();
    if (!target) return false;
    focusTextarea(target.textarea);

    // Un KeyboardEvent JavaScript synthétique n'a pas le droit de lire le
    // presse-papiers. Le pont Android envoie donc le vrai raccourci Ctrl+V au
    // WebView focalisé ; Chromium déclenche alors son événement `paste` natif
    // et xterm reçoit le contenu comme avec un clavier physique.
    var nativeBridge = bridge();
    if (nativeBridge && typeof nativeBridge.pasteFromClipboard === "function") {
      try {
        nativeBridge.pasteFromClipboard();
        disarmModifiers();
        return true;
      } catch (ignored) {
        // Repli utile si une ancienne coque charge ce script pendant une mise
        // à jour : elle reçoit au moins la combinaison Ctrl+V.
      }
    }
    sendShortcut("v", "KeyV", true);
    return true;
  }

  function makeButton(label, title, action, modifierName) {
    var button = document.createElement("button");
    button.type = "button";
    button.className = "cst-term-key";
    button.textContent = label;
    button.title = title;
    button.setAttribute("aria-label", title || label);
    if (modifierName) {
      button.setAttribute("data-cst-modifier", modifierName);
    }
    var lastPointerAction = 0;
    button.addEventListener("pointerdown", function (event) {
      // Garde la focalisation sur le champ xterm : sans preventDefault, le
      // bouton volerait le focus et le clavier virtuel se fermerait.
      event.preventDefault();
      event.stopImmediatePropagation();
      lastPointerAction = Date.now();
      action();
    });
    button.addEventListener("pointerup", function (event) {
      event.preventDefault();
      event.stopImmediatePropagation();
    });
    button.addEventListener("click", function (event) {
      // Android peut produire un click de compatibilité après pointerdown. On
      // l'absorbe pour qu'il ne traverse jamais vers l'onglet Chats sous la
      // barre. Un click clavier/accessibilité reste néanmoins fonctionnel.
      event.preventDefault();
      event.stopImmediatePropagation();
      if (Date.now() - lastPointerAction > 750) action();
    });
    return button;
  }

  function buildToolbar() {
    var toolbar = document.createElement("div");
    toolbar.id = "cst-term-toolbar";
    toolbar.className = "cst-term-toolbar cst-hidden";
    ["pointerdown", "pointerup", "click", "touchstart", "touchend"].forEach(function (type) {
      toolbar.addEventListener(type, function (event) {
        event.stopPropagation();
      });
    });

    var row1 = document.createElement("div");
    row1.className = "cst-term-row";
    row1.appendChild(makeButton("Échap", "Échap", function () { sendWithSticky("Escape", "Escape"); }));
    row1.appendChild(makeButton("Tab", "Tabulation", function () { sendWithSticky("Tab", "Tab"); }));
    row1.appendChild(makeButton("Ctrl", "Ctrl — prochaine touche", function () { armModifier("ctrl"); }, "ctrl"));
    row1.appendChild(makeButton("Alt", "Alt — prochaine touche", function () { armModifier("alt"); }, "alt"));
    row1.appendChild(makeButton("←", "Flèche gauche", function () { sendWithSticky("ArrowLeft", "ArrowLeft"); }));
    row1.appendChild(makeButton("↑", "Flèche haut", function () { sendWithSticky("ArrowUp", "ArrowUp"); }));
    row1.appendChild(makeButton("↓", "Flèche bas", function () { sendWithSticky("ArrowDown", "ArrowDown"); }));
    row1.appendChild(makeButton("→", "Flèche droite", function () { sendWithSticky("ArrowRight", "ArrowRight"); }));
    row1.appendChild(makeButton("Ctrl+V", "Ctrl+V — coller", function () { pasteFromClipboard(); }));
    row1.appendChild(makeButton("Ctrl+C", "Ctrl+C", function () { sendShortcut("c", "KeyC", true); }));
    toolbar.appendChild(row1);

    document.body.appendChild(toolbar);
    state.toolbar = toolbar;
  }

  function refreshToolbarVisibility() {
    if (!state.toolbar) return;
    var show = visibleTerminalHost() !== null && !visibleModal();
    state.toolbar.classList.toggle("cst-hidden", !show);
    if (!show) disarmModifiers();
  }

  function handleTerminalPointerDown(event) {
    var target = event.target;
    var host = target && target.closest ? target.closest("[data-terminal-host]") : null;
    if (!host) return;
    var textarea = host.querySelector(".xterm-helper-textarea");
    focusTextarea(textarea);
    requestKeyboard(true);
  }

  function installStyle() {
    var style = document.createElement("style");
    style.textContent =
      "#cst-term-toolbar{position:fixed;left:0;right:0;bottom:0;z-index:2147483647;" +
      "display:flex;flex-direction:column;gap:4px;padding:6px 6px calc(6px + env(safe-area-inset-bottom));" +
      "background:rgba(10,10,12,.96);border-top:1px solid rgba(255,255,255,.12);" +
      "box-shadow:0 -4px 16px rgba(0,0,0,.45);" +
      "font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;" +
      "-webkit-user-select:none;user-select:none;touch-action:none;isolation:isolate}" +
      "#cst-term-toolbar.cst-hidden{display:none}" +
      ".cst-term-row{display:flex;gap:3px;justify-content:center;flex-wrap:nowrap}" +
      ".cst-term-key{flex:1 1 0;min-width:0;height:40px;border:1px solid rgba(255,255,255,.16);" +
      "border-radius:8px;background:rgba(255,255,255,.08);color:#f2f2f2;font-size:clamp(10px,3vw,14px);font-weight:600;" +
      "position:relative;z-index:1;padding:0 2px;touch-action:none;-webkit-tap-highlight-color:transparent;outline:none}" +
      ".cst-term-key:active{background:rgba(255,255,255,.22)}" +
      ".cst-term-key.cst-active{background:#3b82f6;border-color:#60a5fa;color:#fff}";
    document.head.appendChild(style);
  }

  function init() {
    if (!document.body) {
      document.addEventListener("DOMContentLoaded", init, { once: true });
      return;
    }
    installStyle();
    buildToolbar();
    refreshToolbarVisibility();

    // Le SPA remonte les murs de terminaux sans recharger la page : on garde
    // la visibilité de la barre à jour en continu.
    document.addEventListener("pointerdown", handleTerminalPointerDown, true);
    window.addEventListener("resize", refreshToolbarVisibility);
    if (window.visualViewport) {
      window.visualViewport.addEventListener("resize", refreshToolbarVisibility);
    }
    window.setInterval(refreshToolbarVisibility, 400);
    if (window.MutationObserver) {
      var observer = new MutationObserver(refreshToolbarVisibility);
      observer.observe(document.body, { childList: true, subtree: true });
    }
  }

  init();
})();
