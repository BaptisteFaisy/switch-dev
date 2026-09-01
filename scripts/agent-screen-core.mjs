import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const AGENT_SCREEN_PIPE = "\\\\.\\pipe\\CodexSwitchAgentScreen";
export const MAX_AGENT_SCREEN_REQUEST_BYTES = 65_536;
export const MAX_AGENT_SCREEN_RESPONSE_BYTES = 1_100_000;

// Une capture plein ecran JPEG re-echantillonnee reste sous ce plafond apres
// encodage base64 ; le relais PowerShell diminue la qualite tant qu'il n'y est
// pas. La reponse JSON (base64 + metadonnees) tient ainsi dans la limite de
// ligne du pont SSH et du serveur MCP cote conteneur.
export const MAX_SCREENSHOT_BASE64 = 850_000;

const SESSION_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/;

// Navigation + lancement : les combinaisons systeme a risque (Alt+Tab,
// Ctrl+Alt+Suppr, Ctrl+W, presse-papiers...) restent interdites. Les touches
// supplementaires sont celles que le test de navigation au clavier a montre
// necessaires : F6 (cycler barre d'adresse/contenu), Ctrl+L (focaliser la
// barre d'adresse), Win (menu Demarrer, puis saisie pour lancer une appli).
export const SAFE_SCREEN_KEYS = new Set([
  "Enter",
  "Escape",
  "Tab",
  "Shift+Tab",
  "Backspace",
  "Delete",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Space",
  "F6",
  "Ctrl+L",
  "Win",
]);

// Sequences de codes virtuels Windows (user32 VK_*) : chaque touche est une
// liste de touches a enfoncer dans l'ordre (relachees en sens inverse), ce qui
// permet les modificateurs (Shift+Tab, Ctrl+L).
export const SCREEN_KEY_VK = {
  Enter: [0x0d],
  Escape: [0x1b],
  Tab: [0x09],
  "Shift+Tab": [0x10, 0x09],
  Backspace: [0x08],
  Delete: [0x2e],
  ArrowUp: [0x26],
  ArrowDown: [0x28],
  ArrowLeft: [0x25],
  ArrowRight: [0x27],
  Home: [0x24],
  End: [0x23],
  PageUp: [0x21],
  PageDown: [0x22],
  Space: [0x20],
  F6: [0x75],
  "Ctrl+L": [0x11, 0x4c],
  Win: [0x5b],
};

export const SCREEN_ACTIONS = new Set([
  "health",
  "screenshot",
  "locate",
  "uia_fields",
  "move",
  "click",
  "double_click",
  "right_click",
  "type",
  "press",
  "scroll",
  "windows",
  "open",
  "browser",
  "arm",
  "disarm",
]);

// Methodes navigateur (CDP) et leurs champs autorises.
export const BROWSER_METHODS = new Set([
  "list",
  "screenshot",
  "navigate",
  "eval",
  "click",
  "type",
  "key",
]);
const MAX_BROWSER_URL_LENGTH = 4000;
const MAX_BROWSER_EXPRESSION_LENGTH = 4000;
const MAX_BROWSER_SELECTOR_LENGTH = 500;

const MAX_SCREEN_COORDINATE = 100_000;
const MAX_TYPE_TEXT_LENGTH = 2000;
const MAX_SCROLL_NOTCHES = 20;
const DEFAULT_ARM_MINUTES = 10;
const MAX_ARM_MINUTES = 60;
const MAX_OPEN_COMMAND_LENGTH = 512;
const MAX_WINDOWS_RESULTS = 60;

// Un handle de fenetre Windows est un entier positif (HWND).
const asWindowId = (value, label) => {
  if (value === undefined || value === null || value === "") return undefined;
  if (typeof value !== "number" || !Number.isInteger(value) || value <= 0 || value > 0x7fffffff) {
    throw new Error(`${label} invalide.`);
  }
  return value;
};

const asTrimmedString = (value, label, maximum) => {
  if (typeof value !== "string") throw new Error(`${label} invalide.`);
  const result = value.trim();
  if (!result || result.length > maximum) throw new Error(`${label} invalide.`);
  return result;
};

const asInteger = (value, label, minimum, maximum) => {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw new Error(`${label} invalide.`);
  }
  if (value < minimum || value > maximum) throw new Error(`${label} hors limites.`);
  return value;
};

const optionalCoordinate = (value, label) => {
  if (value === undefined || value === null || value === "") return undefined;
  return asInteger(value, label, -MAX_SCREEN_COORDINATE, MAX_SCREEN_COORDINATE);
};

// Refuse les contenus sensibles : identifiants, mots de passe, codes de
// verification et donnees bancaires ne doivent jamais etre tapes par le chat
// sur l'ecran de l'utilisateur (il les saisit lui-meme). Une suite contigue de
// 13 a 19 chiffres evoque un numero de carte ou un IBAN.
export const isSensitiveText = (value) => {
  const text = String(value || "");
  if (!text.trim()) return false;
  const normalized = text.toLowerCase().replace(/[^a-z0-9]+/g, "-");
  const tokenPattern =
    /(?:^|-)(?:password|passwd|passphrase|passcode|pin|one-time-code|otp|totp|verification-code|security-code|auth-code|cvv|cvc|csc|cid|card-number|cardnumber|credit-card|debit-card|bank-account|account-number|routing-number|sort-code|iban)(?:$|-)/;
  if (tokenPattern.test(normalized)) return true;
  // Equivalents francais couramment employes (mot de passe, mdp).
  const frenchPattern =
    /(?:^|-)(?:mot-de-passe|code-secret|mdp)(?:$|-)/;
  if (frenchPattern.test(normalized)) return true;
  return /\d{13,19}/.test(text.replace(/\s+/g, ""));
};

export const validateAgentScreenRequest = (value) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Demande de controle de l'ecran invalide.");
  }
  if (value.kind !== "agent-screen") {
    throw new Error("Type de demande ecran invalide.");
  }
  const sessionId = asTrimmedString(value.sessionId, "Session ecran", 128);
  if (!SESSION_ID_PATTERN.test(sessionId)) {
    throw new Error("Session ecran invalide.");
  }
  const action = asTrimmedString(value.action, "Action ecran", 32);
  const request = { kind: "agent-screen", sessionId, action };

  switch (action) {
    case "health":
    case "windows":
      break;
    case "screenshot": {
      const window = asWindowId(value.window, "Fenetre cible");
      if (window !== undefined) request.window = window;
      break;
    }
    case "uia_fields": {
      const window = asWindowId(value.window, "Fenetre cible");
      if (window === undefined) {
        throw new Error("Fenetre cible manquante pour uia_fields.");
      }
      request.window = window;
      if (value.index !== undefined && value.index !== null && value.index !== "") {
        request.index = asInteger(value.index, "Index du champ", 0, 99);
      }
      break;
    }
    case "locate": {
      const text = asTrimmedString(value.text, "Texte a repérer", 80);
      if (!text) throw new Error("Le texte a repérer est requis.");
      request.text = text;
      if (value.index !== undefined && value.index !== null && value.index !== "") {
        request.index = asInteger(value.index, "Index du candidat", 0, 50);
      }
      const window = asWindowId(value.window, "Fenetre cible");
      if (window !== undefined) request.window = window;
      break;
    }
    case "move":
    case "click":
    case "double_click":
    case "right_click":
      request.x = asInteger(value.x, "Coordonnee X", -MAX_SCREEN_COORDINATE, MAX_SCREEN_COORDINATE);
      request.y = asInteger(value.y, "Coordonnee Y", -MAX_SCREEN_COORDINATE, MAX_SCREEN_COORDINATE);
      if (action !== "move") {
        const window = asWindowId(value.window, "Fenetre cible");
        if (window !== undefined) request.window = window;
      }
      break;
    case "type": {
      if (typeof value.text !== "string" || !value.text.trim()) {
        throw new Error("Le texte a saisir est invalide.");
      }
      if (value.text.length > MAX_TYPE_TEXT_LENGTH) {
        throw new Error("Le texte a saisir est trop long.");
      }
      if (/[\u0000-\u001f\u007f]/.test(value.text)) {
        throw new Error("Le texte a saisir contient des caracteres de controle.");
      }
      if (isSensitiveText(value.text)) {
        throw new Error(
          "Switch ne saisit pas de mot de passe, code de verification ou donnee bancaire sur l'ecran. Saisissez-les vous-meme."
        );
      }
      request.text = value.text;
      // mode "messages" (defaut) : WM_CHAR cible, marche sur les apps Win32
      // classiques. mode "focus" : focus clavier temporaire par
      // AttachThreadInput (ordre Z intact) — accepte partout, y compris
      // Chromium/WebView2 inactif.
      if (value.mode !== undefined && value.mode !== null && value.mode !== "") {
        const mode = asTrimmedString(value.mode, "Mode de saisie", 16);
        if (mode !== "messages" && mode !== "focus" && mode !== "uia") {
          throw new Error("Mode de saisie invalide (messages, focus ou uia)." );
        }
        request.mode = mode;
      }
      if (value.uiaIndex !== undefined && value.uiaIndex !== null && value.uiaIndex !== "") {
        request.uiaIndex = asInteger(value.uiaIndex, "Index UIA", 0, 99);
      }
      const window = asWindowId(value.window, "Fenetre cible");
      if (window !== undefined) request.window = window;
      if (request.mode === "uia" && request.window === undefined) {
        throw new Error("La saisie mode uia exige une fenetre cible (window).");
      }
      break;
    }
    case "press":
      request.key = asTrimmedString(value.key, "Touche", 32);
      if (!SAFE_SCREEN_KEYS.has(request.key)) {
        throw new Error("Cette touche n'est pas autorisee sur l'ecran.");
      }
      if (value.mode !== undefined && value.mode !== null && value.mode !== "") {
        const mode = asTrimmedString(value.mode, "Mode de saisie", 16);
        if (mode !== "messages" && mode !== "focus") {
          throw new Error("Mode de saisie invalide (messages ou focus)." );
        }
        request.mode = mode;
      }
      {
        const window = asWindowId(value.window, "Fenetre cible");
        if (window !== undefined) request.window = window;
      }
      break;
    case "arm":
      if (value.minutes === undefined || value.minutes === null || value.minutes === "") {
        request.minutes = DEFAULT_ARM_MINUTES;
      } else {
        request.minutes = asInteger(value.minutes, "Duree d'armement", 1, MAX_ARM_MINUTES);
      }
      break;
    case "disarm":
      break;
    case "scroll": {
      const amount = asInteger(value.amount, "Quantite de defilement", -MAX_SCROLL_NOTCHES, MAX_SCROLL_NOTCHES);
      if (amount === 0) throw new Error("Quantite de defilement invalide.");
      request.amount = amount;
      const x = optionalCoordinate(value.x, "Coordonnee X");
      const y = optionalCoordinate(value.y, "Coordonnee Y");
      if (x !== undefined || y !== undefined) {
        if (x === undefined || y === undefined) {
          throw new Error("Coordonnees de defilement incompletes.");
        }
        request.x = x;
        request.y = y;
      }
      const window = asWindowId(value.window, "Fenetre cible");
      if (window !== undefined) request.window = window;
      break;
    }
    case "open": {
      const command = asTrimmedString(value.command, "Commande", MAX_OPEN_COMMAND_LENGTH);
      if (/[\u0000-\u001f\u007f]/.test(command)) {
        throw new Error("La commande contient des caracteres de controle.");
      }
      request.command = command;
      // hidden : helper (script console) lance sans aucune fenetre visible —
      // evite les terminaux oublies ouvert sur le bureau de l'utilisateur.
      if (value.hidden === true) request.hidden = true;
      break;
    }
    case "browser": {
      const method = asTrimmedString(value.method, "Methode navigateur", 16);
      if (!BROWSER_METHODS.has(method)) {
        throw new Error("Methode navigateur non prise en charge.");
      }
      request.method = method;
      // Port CDP explicite : permet de piloter le WebView2 de l'app elle-meme
      // (active par CST_WEBVIEW_CDP_PORT au lancement) sans fenetre lancee.
      if (value.cdpPort !== undefined && value.cdpPort !== null && value.cdpPort !== "") {
        request.cdpPort = asInteger(value.cdpPort, "Port CDP", 1024, 65535);
      }
      const window = asWindowId(value.window, "Fenetre navigateur");
      if (window === undefined && request.cdpPort === undefined) {
        throw new Error("Fenetre navigateur manquante (id de screen_windows) — ou passez cdpPort.");
      }
      if (window !== undefined) request.window = window;
      if (method === "navigate") {
        const url = asTrimmedString(value.url, "URL", MAX_BROWSER_URL_LENGTH);
        if (!/^https?:\/\//i.test(url)) {
          throw new Error("URL invalide (http ou https attendu).");
        }
        request.url = url;
      } else if (method === "eval") {
        const expression = asTrimmedString(value.expression, "Expression", MAX_BROWSER_EXPRESSION_LENGTH);
        request.expression = expression;
      } else if (method === "click") {
        request.selector = asTrimmedString(value.selector, "Selecteur", MAX_BROWSER_SELECTOR_LENGTH);
      } else if (method === "type") {
        request.selector = asTrimmedString(value.selector, "Selecteur", MAX_BROWSER_SELECTOR_LENGTH);
        if (typeof value.text !== "string" || !value.text.trim()) {
          throw new Error("Le texte a saisir dans la page est invalide.");
        }
        if (value.text.length > MAX_TYPE_TEXT_LENGTH) {
          throw new Error("Le texte a saisir dans la page est trop long.");
        }
        if (/[\u0000-\u001f\u007f]/.test(value.text)) {
          throw new Error("Le texte a saisir dans la page contient des caracteres de controle.");
        }
        if (isSensitiveText(value.text)) {
          throw new Error(
            "Switch ne saisit pas de mot de passe, code de verification ou donnee bancaire, meme dans une page web."
          );
        }
        request.text = value.text;
      } else if (method === "key") {
        const key = asTrimmedString(value.key, "Touche navigateur", 32);
        if (!SAFE_SCREEN_KEYS.has(key) || key === "Win") {
          throw new Error("Cette touche n'est pas autorisee dans la page.");
        }
        request.key = key;
      }
      break;
    }
    default:
      throw new Error("Action ecran non prise en charge.");
  }
  return request;
};

// Confirmation Windows locale pour les actions mutantes : un clic peut toucher
// un bouton sensible (paiement, suppression), une saisie peut aboutir dans une
// fenetre non prevue. Le modele et la page ne peuvent pas approuver eux-memes ;
// seule la personne presente au clavier peut valider. Meme mecanique que le
// navigateur agent (MessageBox PowerShell encodee).
export const requiresScreenApproval = (action, key = "") => {
  // L'armement ne demande AUCUNE popup : il n'est valide qu'apres une demande
  // explicite de l'utilisateur dans le chat (les popups Windows lancees depuis
  // un processus en arriere-plan ne s'affichent pas de facon fiable).
  if (action.startsWith("browser:")) {
    // Lecture seule (liste des onglets, capture de la page) : aucune
    // confirmation. Navigation, evaluation JS, clic, saisie, touche : ce sont
    // des mutations de la page — confirmation ou session armee.
    const method = action.slice("browser:".length);
    return method !== "list" && method !== "screenshot";
  }
  if (action === "click" || action === "double_click" || action === "right_click" || action === "type") {
    return true;
  }
  if (action === "press") return ["Enter", "Space"].includes(key);
  return false;
};

export const confirmWindowsScreenAction = ({ action, x, y, key = "", text = "" }) =>
  new Promise((resolveApproval) => {
    if (!requiresScreenApproval(action, key)) {
      resolveApproval(true);
      return;
    }
    const details = [];
    if (typeof x === "number" && typeof y === "number") {
      details.push(`Position : (${x}, ${y})`);
    }
    if (key) details.push(`Touche : ${key}`);
    if (text) details.push(`Texte : ${String(text).slice(0, 240)}`);
    const description = [
      `Action demandee par Switch : ${action}`,
      ...details,
      "",
      "Autoriser cette action sur l'ecran ?",
    ].join("\r\n");
    const message = Buffer.from(description, "utf8").toString("base64");
    // Fenetre TopMost : un MessageBox lance depuis un processus en arriere-plan
    // reste derriere la fenetre active (anti-vol de focus Windows) et finit par
    // expirer sans etre vu. Seul un clic explicite sur « Oui » approuve : Entree
    // et Echap refusent (bouton par defaut = Non).
    const script = [
      "Add-Type -AssemblyName System.Windows.Forms",
      "Add-Type -AssemblyName System.Drawing",
      `$message=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${message}'))`,
      "$form = New-Object Windows.Forms.Form",
      "$form.Text = 'Switch - confirmation ecran'",
      "$form.TopMost = $true",
      "$form.StartPosition = 'CenterScreen'",
      "$form.FormBorderStyle = 'FixedDialog'",
      "$form.MaximizeBox = $false",
      "$form.MinimizeBox = $false",
      "$form.ClientSize = New-Object Drawing.Size(580, 240)",
      "$label = New-Object Windows.Forms.Label",
      "$label.Text = $message",
      "$label.Left = 20; $label.Top = 16; $label.Width = 540; $label.Height = 160",
      "$label.Font = New-Object Drawing.Font('Segoe UI', 10)",
      "$yes = New-Object Windows.Forms.Button",
      "$yes.Text = 'Oui'",
      "$yes.Left = 370; $yes.Top = 190; $yes.Width = 95; $yes.Height = 32",
      "$no = New-Object Windows.Forms.Button",
      "$no.Text = 'Non'",
      "$no.Left = 475; $no.Top = 190; $no.Width = 95; $no.Height = 32",
      "$form.Controls.Add($label); $form.Controls.Add($yes); $form.Controls.Add($no)",
      "$yes.Add_Click({ $form.DialogResult = 'Yes'; $form.Close() })",
      "$no.Add_Click({ $form.DialogResult = 'No'; $form.Close() })",
      "$form.AcceptButton = $no",
      "$form.CancelButton = $no",
      "$result = $form.ShowDialog()",
      "if ($result -ne 'Yes') { exit 3 }",
    ].join("; ");
    const encoded = Buffer.from(script, "utf16le").toString("base64");
    const child = spawn("powershell.exe", [
      "-NoProfile",
      "-STA",
      "-NonInteractive",
      "-EncodedCommand",
      encoded,
    ], { stdio: "ignore", windowsHide: true });
    let settled = false;
    const finish = (approved) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolveApproval(approved);
    };
    const timer = setTimeout(() => {
      child.kill();
      finish(false);
    }, 25_000);
    timer.unref?.();
    child.once("error", () => finish(false));
    child.once("close", (code) => finish(code === 0));
  });

// Script PowerShell unique (compile une fois par process) qui execute l'action
// demandee : capture d'ecran re-echantillonnee en JPEG base64, souris
// (SendInput), clavier (SendInput Unicode). La requete arrive en base64 sur
// stdin, la reponse JSON part sur stdout en UTF-8. Aucune dependance native :
// uniquement .NET de base (System.Drawing) et user32 via P/Invoke.
export const buildAgentScreenPowerShellScript = () => String.raw`
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$MAX_SCREENSHOT_BASE64 = 850000
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ScreenAgent
{
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION
    {
        [FieldOffset(0)] public KEYBDINPUT ki;
        [FieldOffset(0)] public MOUSEINPUT mi;
    }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT
    {
        public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT
    {
        public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION U; }
    [StructLayout(LayoutKind.Sequential)] public struct CURSORINFO
    {
        public int cbSize; public int flags; public IntPtr hCursor; public POINT ptScreenPos;
    }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct STARTUPINFO
    {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX; public int dwY; public int dwXSize; public int dwYSize;
        public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute;
        public int dwFlags; public short wShowWindow; public short cbReserved2;
        public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
    }
    [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION
    {
        public IntPtr hProcess; public IntPtr hThread; public uint dwProcessId; public uint dwThreadId;
    }
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT lpPoint);
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
    [DllImport("user32.dll")] public static extern bool GetCursorInfo(ref CURSORINFO pci);
    [DllImport("user32.dll")] public static extern bool DrawIcon(IntPtr hDC, int X, int Y, IntPtr hIcon);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int nIndex);
    public const uint INPUT_MOUSE = 0;
    public const uint INPUT_KEYBOARD = 1;
    public const uint KEYEVENTF_UNICODE = 0x0004;
    public const uint KEYEVENTF_KEYUP = 0x0002;
    public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    public const uint MOUSEEVENTF_LEFTUP = 0x0004;
    public const uint MOUSEEVENTF_RIGHTDOWN = 0x0008;
    public const uint MOUSEEVENTF_RIGHTUP = 0x0010;
    public const uint MOUSEEVENTF_WHEEL = 0x0800;
    public const int SM_XVIRTUALSCREEN = 76;
    public const int SM_YVIRTUALSCREEN = 77;
    public const int SM_CXVIRTUALSCREEN = 78;
    public const int SM_CYVIRTUALSCREEN = 79;
    public const uint WM_KEYDOWN = 0x0100;
    public const uint WM_KEYUP = 0x0101;
    public const uint WM_CHAR = 0x0102;
    public const uint WM_GETOBJECT = 0x003D;
    public const uint WM_LBUTTONDOWN = 0x0201;
    public const uint WM_LBUTTONUP = 0x0202;
    public const uint WM_LBUTTONDBLCLK = 0x0203;
    public const uint WM_RBUTTONDOWN = 0x0204;
    public const uint WM_RBUTTONUP = 0x0205;
    public const uint WM_MOUSEWHEEL = 0x020A;
    public const uint MK_LBUTTON = 0x0001;
    public const uint MK_RBUTTON = 0x0002;
    public const uint PW_RENDERFULLCONTENT = 0x00000002;
    public const uint STARTF_USESHOWWINDOW = 0x00000001;
    public const uint STARTF_USESTDHANDLES = 0x00000100;
    public const uint GENERIC_WRITE = 0x40000000;
    public const uint GENERIC_READ = 0x80000000;
    public const uint FILE_SHARE_READ = 0x00000001;
    public const uint FILE_SHARE_WRITE = 0x00000002;
    public const uint OPEN_EXISTING = 3;
    public const short SW_SHOWNOACTIVATE = 4;
    [StructLayout(LayoutKind.Sequential)] public struct SECURITY_ATTRIBUTES { public int nLength; public IntPtr lpSecurityDescriptor; public bool bInheritHandle; }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess, uint dwShareMode, ref SECURITY_ATTRIBUTES lpSecurityAttributes, uint dwCreationDisposition, uint dwFlagsAndAttributes, IntPtr hTemplateFile);

    public static bool Move(int x, int y) { return SetCursorPos(x, y); }
    public static int CursorX() { POINT p; GetCursorPos(out p); return p.X; }
    public static int CursorY() { POINT p; GetCursorPos(out p); return p.Y; }
    public static int VirtualLeft() { return GetSystemMetrics(SM_XVIRTUALSCREEN); }
    public static int VirtualTop() { return GetSystemMetrics(SM_YVIRTUALSCREEN); }
    public static int VirtualWidth() { return GetSystemMetrics(SM_CXVIRTUALSCREEN); }
    public static int VirtualHeight() { return GetSystemMetrics(SM_CYVIRTUALSCREEN); }

    private static void Mouse(uint flags)
    {
        INPUT[] inputs = new INPUT[1];
        inputs[0].type = INPUT_MOUSE;
        inputs[0].U.mi.dwFlags = flags;
        inputs[0].U.mi.time = 0;
        SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    public static void Click(int x, int y, bool right, bool twice)
    {
        SetCursorPos(x, y);
        uint down = right ? MOUSEEVENTF_RIGHTDOWN : MOUSEEVENTF_LEFTDOWN;
        uint up = right ? MOUSEEVENTF_RIGHTUP : MOUSEEVENTF_LEFTUP;
        int count = twice ? 2 : 1;
        for (int i = 0; i < count; i++)
        {
            Mouse(down);
            Mouse(up);
            if (twice) System.Threading.Thread.Sleep(50);
        }
    }

    public static void Scroll(int notches)
    {
        int delta = notches * 120;
        INPUT[] inputs = new INPUT[1];
        inputs[0].type = INPUT_MOUSE;
        inputs[0].U.mi.dwFlags = MOUSEEVENTF_WHEEL;
        inputs[0].U.mi.mouseData = (uint)delta;
        inputs[0].U.mi.time = 0;
        SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    public static void Key(ushort vk, bool up)
    {
        INPUT[] inputs = new INPUT[1];
        inputs[0].type = INPUT_KEYBOARD;
        inputs[0].U.ki.wVk = vk;
        inputs[0].U.ki.dwFlags = up ? KEYEVENTF_KEYUP : 0;
        inputs[0].U.ki.time = 0;
        SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    public static void PressSequence(int[] vks)
    {
        if (vks == null || vks.Length == 0) return;
        for (int i = 0; i < vks.Length; i++) Key((ushort)vks[i], false);
        for (int i = vks.Length - 1; i >= 0; i--) Key((ushort)vks[i], true);
    }

    public static void Type(string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        INPUT[] inputs = new INPUT[text.Length * 2];
        int index = 0;
        foreach (char c in text)
        {
            inputs[index].type = INPUT_KEYBOARD;
            inputs[index].U.ki.wVk = 0;
            inputs[index].U.ki.wScan = (ushort)c;
            inputs[index].U.ki.dwFlags = KEYEVENTF_UNICODE;
            index += 1;
            inputs[index].type = INPUT_KEYBOARD;
            inputs[index].U.ki.wVk = 0;
            inputs[index].U.ki.wScan = (ushort)c;
            inputs[index].U.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
            index += 1;
        }
        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    public static string ForegroundTitle()
    {
        IntPtr window = GetForegroundWindow();
        if (window == IntPtr.Zero) return "";
        StringBuilder builder = new StringBuilder(512);
        GetWindowText(window, builder, builder.Capacity);
        return builder.ToString();
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern bool CreateProcess(string lpApplicationName, string lpCommandLine, IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles, uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory, ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
    [DllImport("kernel32.dll")] public static extern void CloseHandle(IntPtr hObject);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hWnd, EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);

    // Activation de l'accessibilite Chromium : WM_GETOBJECT/OBJID_CLIENT est
    // le signal qui fait construire l'arbre UIA par le renderer (idempotent).
    public static void SendGetObject(long hwnd)
    {
        SendMessage(new IntPtr(hwnd), WM_GETOBJECT, IntPtr.Zero, (IntPtr)(-4));
    }

    // Handles des fenetres enfants de classe Chromium (WebView2/Chrome) — les
    // cibles du WM_GETOBJECT d'activation.
    public static string[] ChromiumChildHandles(long hwnd)
    {
        var results = new System.Collections.Generic.List<string>();
        EnumChildWindows(new IntPtr(hwnd), (h, l) =>
        {
            var sb = new StringBuilder(128);
            GetClassName(h, sb, 128);
            string cls = sb.ToString();
            if (cls.Contains("Chrome")) results.Add(h.ToInt64().ToString(System.Globalization.CultureInfo.InvariantCulture) + "\u001F" + cls);
            return true;
        }, IntPtr.Zero);
        return results.ToArray();
    }

    // Resout le chemin complet d'un executable : si le premier mot contient un
    // separateur de chemin, il est utilise tel quel ; sinon on cherche dans le
    // PATH puis dans les dossiers d'installation courants (Chrome, Edge,
    // Firefox, Notepad). Retourne le chemin complet ou null.
    public static string ResolveExe(string commandLine)
    {
        string first = commandLine.TrimStart().Split(' ')[0].Trim('"');
        if (first.Contains("\\") || first.Contains("/")) return first;
        string exe = first.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? first : first + ".exe";
        string path = Environment.GetEnvironmentVariable("PATH") ?? "";
        foreach (string dir in path.Split(';'))
        {
            if (string.IsNullOrEmpty(dir)) continue;
            string candidate = System.IO.Path.Combine(dir.Trim('"'), exe);
            if (System.IO.File.Exists(candidate)) return candidate;
        }
        string pf = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
        string pfx86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
        string[] dirs = new string[] {
            pf + "\\Google\\Chrome\\Application", pfx86 + "\\Google\\Chrome\\Application",
            pf + "\\Microsoft\\Edge\\Application", pfx86 + "\\Microsoft\\Edge\\Application",
            pf + "\\Mozilla Firefox", pfx86 + "\\Mozilla Firefox",
            pf + "\\Windows NT\\Accessories", pf + "\\WindowsApps"
        };
        foreach (string dir in dirs)
        {
            if (string.IsNullOrEmpty(dir)) continue;
            string candidate = System.IO.Path.Combine(dir, exe);
            if (System.IO.File.Exists(candidate)) return candidate;
        }
        return null;
    }

    // Lance une application SANS prendre le focus : la fenetre principale est
    // creee visible mais NON activee (SW_SHOWNOACTIVATE) — le premier plan de
    // l'utilisateur reste intact. L'agent pilote ensuite cette fenetre par
    // PostMessage/PrintWindow, en arriere-plan.
    public static bool LaunchNoFocus(string commandLine, bool hidden)
    {
        if (string.IsNullOrWhiteSpace(commandLine)) return false;
        string exe = ResolveExe(commandLine);
        if (exe == null) return false;
        // Premier mot remplace par le chemin complet, le reste des arguments
        // (y compris les guillemets) est conserve tel quel.
        int firstArg = commandLine.TrimStart().IndexOf(' ');
        string full = firstArg < 0 ? exe : exe + commandLine.TrimStart().Substring(firstArg);
        STARTUPINFO si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
        si.dwFlags = (int)STARTF_USESHOWWINDOW;
        // hidden : helper console sans AUCUNE fenetre (CREATE_NO_WINDOW + SW_HIDE)
        // — rien a fermer ensuite ; sinon fenetre visible mais NON activee
        // (SW_SHOWNOACTIVATE) : le premier plan de l'utilisateur reste intact.
        si.wShowWindow = hidden ? (short)0 : SW_SHOWNOACTIVATE;
        // stdio de l'enfant redirige vers NUL : une app console (cmd...) ne
        // doit jamais ecrire dans le pipe de reponse du relais PowerShell.
        IntPtr nullHandle = IntPtr.Zero;
        var sa = new SECURITY_ATTRIBUTES { nLength = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)), lpSecurityDescriptor = IntPtr.Zero, bInheritHandle = true };
        nullHandle = CreateFileW("NUL", GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, ref sa, OPEN_EXISTING, 0, IntPtr.Zero);
        if (nullHandle != IntPtr.Zero && nullHandle != (IntPtr)(-1))
        {
            si.dwFlags |= (int)STARTF_USESTDHANDLES;
            si.hStdInput = nullHandle;
            si.hStdOutput = nullHandle;
            si.hStdError = nullHandle;
        }
        PROCESS_INFORMATION pi;
        bool created = CreateProcess(null, full, IntPtr.Zero, IntPtr.Zero, nullHandle != IntPtr.Zero && nullHandle != (IntPtr)(-1), hidden ? 0x08000000u : 0u, IntPtr.Zero, null, ref si, out pi);
        if (nullHandle != IntPtr.Zero && nullHandle != (IntPtr)(-1)) CloseHandle(nullHandle);
        if (created)
        {
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        return created;
    }

    // Liste des fenetres visibles avec titre, dans le format
    // hwnd|pid|titre|left|top|right|bottom (separateur \u001f).
    public static string[] ListWindowsText()
    {
        var results = new System.Collections.Generic.List<string>();
        EnumWindows((hWnd, lParam) =>
        {
            if (!IsWindowVisible(hWnd)) return true;
            var builder = new StringBuilder(512);
            GetWindowText(hWnd, builder, builder.Capacity);
            if (builder.Length == 0) return true;
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            RECT r;
            GetWindowRect(hWnd, out r);
            results.Add(hWnd.ToInt64().ToString() + "\u001f" + pid.ToString() + "\u001f" + builder.ToString() + "\u001f"
                + r.Left.ToString() + "\u001f" + r.Top.ToString() + "\u001f" + r.Right.ToString() + "\u001f" + r.Bottom.ToString());
            return true;
        }, IntPtr.Zero);
        return results.ToArray();
    }

    public static string WindowRectText(long hwnd)
    {
        RECT r;
        if (!GetWindowRect(new IntPtr(hwnd), out r)) return "0|0|0|0";
        return r.Left.ToString() + "|" + r.Top.ToString() + "|" + r.Right.ToString() + "|" + r.Bottom.ToString();
    }

    public static long WindowProcessId(long hwnd)
    {
        uint pid;
        GetWindowThreadProcessId(new IntPtr(hwnd), out pid);
        return pid;
    }

    public static bool PrintWindowEx(long hwnd, IntPtr hdcBlt)
    {
        return PrintWindow(new IntPtr(hwnd), hdcBlt, PW_RENDERFULLCONTENT);
    }

    public static int MakeLParam(int lo, int hi)
    {
        return (hi << 16) | (lo & 0xFFFF);
    }

    // Saisie ciblee : WM_CHAR envoye DIRECTEMENT a la fenetre, sans focus.
    public static void TypeToWindow(long hwnd, string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        IntPtr h = new IntPtr(hwnd);
        foreach (char c in text)
        {
            SendMessage(h, WM_CHAR, (IntPtr)c, IntPtr.Zero);
        }
    }

    // Touche ciblee : WM_KEYDOWN/WM_KEYUP par sequence, sans focus.
    // SendMessage (synchrone) plutot que PostMessage : Chrome traite la
    // touche dans son thread et renvoie — indispensable pour un arriere-plan
    // fiable (PostMessage est ignore par certains controles).
    public static void PressToWindow(long hwnd, int[] vks)
    {
        if (vks == null || vks.Length == 0) return;
        IntPtr h = new IntPtr(hwnd);
        for (int i = 0; i < vks.Length; i++) SendMessage(h, WM_KEYDOWN, (IntPtr)vks[i], IntPtr.Zero);
        for (int i = vks.Length - 1; i >= 0; i--) SendMessage(h, WM_KEYUP, (IntPtr)vks[i], IntPtr.Zero);
    }

    // Saisie par focus temporaire : le focus clavier de la fenetre cible est
    // obtenu par AttachThreadInput + SetFocus SANS changer l'ordre Z (la
    // fenetre de premier plan de l'utilisateur reste affichee), le texte part
    // par SendInput (accepte partout ou un clavier normal marche), puis le
    // focus est rendu a la fenetre de premier plan. Contourne l'ignorance de
    // WM_CHAR par Chromium/WebView2 inactif. Le focus clavier de l'utilisateur
    // est deplace pendant quelques centaines de millisecondes seulement.
    private static void WithTargetFocus(long hwnd, Action send)
    {
        IntPtr h = new IntPtr(hwnd);
        IntPtr fg = GetForegroundWindow();
        uint fgThread = 0;
        uint curThread = GetCurrentThreadId();
        if (fg != IntPtr.Zero)
        {
            uint tmp = 0;
            fgThread = GetWindowThreadProcessId(fg, out tmp);
        }
        uint dummy = 0;
        uint tgtThread = GetWindowThreadProcessId(h, out dummy);
        bool attachedFg = false, attachedTgt = false;
        try
        {
            if (fgThread != 0 && fgThread != curThread) attachedFg = AttachThreadInput(curThread, fgThread, true);
            if (tgtThread != 0 && tgtThread != curThread && tgtThread != fgThread) attachedTgt = AttachThreadInput(curThread, tgtThread, true);
            SetFocus(h);
            System.Threading.Thread.Sleep(30);
            send();
            System.Threading.Thread.Sleep(30);
        }
        finally
        {
            // Rend le focus clavier a la fenetre de premier plan, puis detache.
            if (fg != IntPtr.Zero) SetFocus(fg);
            if (attachedTgt) AttachThreadInput(curThread, tgtThread, false);
            if (attachedFg) AttachThreadInput(curThread, fgThread, false);
        }
    }

    public static void TypeFocusSteal(long hwnd, string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        WithTargetFocus(hwnd, () => Type(text));
    }

    public static void PressFocusSteal(long hwnd, int[] vks)
    {
        if (vks == null || vks.Length == 0) return;
        WithTargetFocus(hwnd, () => PressSequence(vks));
    }

    // Clic cible : coordonnees CLIENT (0,0 = coin haut-gauche de la fenetre),
    // envoye par messages sans focus ni deplacement de la souris.
    public static void ClickWindowClient(long hwnd, int x, int y, bool right, bool twice)
    {
        IntPtr h = new IntPtr(hwnd);
        int lparam = MakeLParam(x, y);
        uint down = right ? WM_RBUTTONDOWN : WM_LBUTTONDOWN;
        uint up = right ? WM_RBUTTONUP : WM_LBUTTONUP;
        IntPtr key = (IntPtr)(right ? MK_RBUTTON : MK_LBUTTON);
        int count = twice ? 2 : 1;
        for (int i = 0; i < count; i++)
        {
            SendMessage(h, down, key, (IntPtr)lparam);
            SendMessage(h, up, IntPtr.Zero, (IntPtr)lparam);
            if (twice) System.Threading.Thread.Sleep(50);
        }
    }

    // Defilement cible : x/y en coordonnees CLIENT ; -1 = centre de la fenetre.
    public static void ScrollWindowClient(long hwnd, int notches, int x, int y)
    {
        IntPtr h = new IntPtr(hwnd);
        if (x < 0 || y < 0)
        {
            RECT r;
            GetClientRect(h, out r);
            x = r.Right / 2;
            y = r.Bottom / 2;
        }
        int lparam = MakeLParam(x, y);
        int delta = notches * 120;
        SendMessage(h, WM_MOUSEWHEEL, (IntPtr)((long)delta << 16), (IntPtr)lparam);
    }

    public static int[] GetVkSequence(string key)
    {
        // SCREEN_KEY_VK_MAPPING
        return null;
    }
}
'@
Add-Type -AssemblyName System.Drawing

# Relais persistant : le processus PowerShell reste vivant entre les requetes.
# Chaque requete arrive en base64 sur une seule ligne de stdin, chaque reponse
# sort en JSON sur une seule ligne de stdout (Write-Result termine par un retour
# ligne + flush explicite). La compilation C# (Add-Type) n'a ainsi lieu qu'une
# seule fois par processus : plus aucun delai de demarrage par action de l'agent.
$script:relayIn = New-Object IO.StreamReader([Console]::OpenStandardInput())
$script:relayOut = [Console]::OpenStandardOutput()

function Write-Result([hashtable]$value) {
    $payload = $value | ConvertTo-Json -Compress -Depth 8
    $bytes = [Text.Encoding]::UTF8.GetBytes($payload)
    $script:relayOut.Write($bytes, 0, $bytes.Length)
    $script:relayOut.WriteByte(10)
    $script:relayOut.Flush()
}

while ($true) {
    $line = $script:relayIn.ReadLine()
    if ($null -eq $line) { break }
    $line = $line.Trim()
    if (-not $line) { continue }
    $json = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($line))
    $request = $json | ConvertFrom-Json


function Convert-Bitmap([Drawing.Bitmap]$sourceBitmap, [int]$sourceWidth, [int]$sourceHeight) {
    $maxDimension = 1600
    $largest = [Math]::Max($sourceWidth, $sourceHeight)
    $scale = [Math]::Min(1.0, ($maxDimension / $largest))
    $targetWidth = [int][Math]::Max(1, [Math]::Round($sourceWidth * $scale))
    $targetHeight = [int][Math]::Max(1, [Math]::Round($sourceHeight * $scale))
    $scaled = New-Object Drawing.Bitmap($targetWidth, $targetHeight)
    $scaledGraphics = [Drawing.Graphics]::FromImage($scaled)
    try {
        $scaledGraphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $scaledGraphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
        $scaledGraphics.DrawImage($sourceBitmap, 0, 0, $targetWidth, $targetHeight)
    } finally {
        $scaledGraphics.Dispose()
    }
    $encoder = [Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.MimeType -eq 'image/jpeg' }
    $encoded = ''
    foreach ($quality in @(72, 55, 40)) {
        $parameters = New-Object Drawing.Imaging.EncoderParameters(1)
        $parameters.Param[0] = New-Object Drawing.Imaging.EncoderParameter([Drawing.Imaging.Encoder]::Quality, [long]$quality)
        $stream = New-Object IO.MemoryStream
        try {
            $scaled.Save($stream, $encoder, $parameters)
            $candidate = [Convert]::ToBase64String($stream.ToArray())
            if ($candidate.Length -le $MAX_SCREENSHOT_BASE64) { $encoded = $candidate; break }
        } finally {
            $stream.Dispose()
            $parameters.Dispose()
        }
    }
    if (-not $encoded) {
        $encoded = [Convert]::ToBase64String([byte[]]@(0xff, 0xd8, 0xff, 0xd9))
    }
    return @{ base64 = $encoded; width = $targetWidth; height = $targetHeight; screenWidth = $sourceWidth; screenHeight = $sourceHeight }
}

# --- Pilotage navigateur (Chrome/Edge) en arriere-plan via CDP ---
# Le clavier WM ne peut pas piloter Chrome quand la fenetre est inactive (son
# focus manager interne ignore les messages). Pour travailler en arriere-plan
# sans voler le focus, on lance Chrome avec un profil isole et un port de
# debug, puis on pilote la page par le Chrome DevTools Protocol : navigation,
# clics JS, saisie, capture reelle de la page — independant du focus Windows.

function Get-CdpPort([long]$window) {
    $procId = [ScreenAgent]::WindowProcessId($window)
    if ($procId -le 0) { throw 'Fenetre cible invalide pour le navigateur.' }
    $proc = Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -eq $procId } | Select-Object -First 1
    if (-not $proc -or -not $proc.CommandLine) { throw 'Processus navigateur introuvable.' }
    $cmd = $proc.CommandLine
    if ($cmd -notmatch '--remote-debugging-port=\d+') { throw 'Navigateur lance sans port de debug : relancez-le avec screen_open.' }
    if ($cmd -notmatch '--user-data-dir="([^"]+)"') {
        if ($cmd -notmatch '--user-data-dir=([^\s]+)') { throw 'Profil navigateur introuvable.' }
    }
    $profile = $Matches[1]
    $portFile = Join-Path $profile 'DevToolsActivePort'
    if (-not (Test-Path $portFile)) { throw 'Port DevTools introuvable (profil non initialise).' }
    $port = ((Get-Content $portFile -TotalCount 1) | Select-Object -First 1).Trim()
    if ($port -notmatch '^\d+$') { throw 'Port DevTools illisible.' }
    return [int]$port
}

function Get-CdpPageTarget([int]$port) {
    $targets = Invoke-RestMethod -Uri ('http://127.0.0.1:{0}/json/list' -f $port) -TimeoutSec 10
    $pages = @($targets | Where-Object { $_.type -eq 'page' })
    if ($pages.Count -eq 0) { throw 'Aucun onglet navigateur disponible.' }
    return $pages[0]
}

# Appel JSON-RPC CDP sur le WebSocket DevTools, avec attente de la reponse
# portant le meme id (les evenements CDP arrivent en parallele).
function Invoke-Cdp([string]$wsUrl, [string]$method, $params) {
    $ws = New-Object System.Net.WebSockets.ClientWebSocket
    try {
        $ws.Options.SetRequestHeader('Origin', 'http://localhost')
        try {
            $connect = $ws.ConnectAsync([Uri]$wsUrl, [Threading.CancellationToken]::None)
            if (-not $connect.Wait(10000)) { throw 'Connexion DevTools expiree.' }
        } catch {
            $inner = $_.Exception.InnerException
            while ($inner -and $inner.InnerException) { $inner = $inner.InnerException }
            throw ('Connexion DevTools echouee : ' + $(if ($inner) { $inner.Message } else { $_.Exception.Message }))
        }
        $id = Get-Random -Minimum 100000 -Maximum 999999999
        $payload = @{ id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 12 -Compress
        $bytes = [Text.Encoding]::UTF8.GetBytes($payload)
        try {
            $send = $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [Threading.CancellationToken]::None)
            if (-not $send.Wait(10000)) { throw 'Envoi DevTools expire.' }
        } catch {
            $inner = $_.Exception.InnerException
            while ($inner -and $inner.InnerException) { $inner = $inner.InnerException }
            throw ('Envoi DevTools echoue : ' + $(if ($inner) { $inner.Message } else { $_.Exception.Message }))
        }
        $buffer = New-Object byte[] 4194304
        $builder = New-Object Text.StringBuilder
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        $response = $null
        while ([DateTime]::UtcNow -lt $deadline) {
            $receive = $ws.ReceiveAsync([ArraySegment[byte]]::new($buffer), [Threading.CancellationToken]::None)
            if (-not $receive.Wait(1000)) { continue }
            $result = $receive.GetAwaiter().GetResult()
            if ($result.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { break }
            [void]$builder.Append([Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count))
            if ($builder.Length -gt 8388608) { throw 'Reponse DevTools trop volumineuse.' }
            # La reponse peut arriver en plusieurs fragments : attendre un JSON
            # COMPLET portant notre id (l'id apparait au debut du premier
            # fragment, avant le base64 de l'image).
            try {
                $candidate = $builder.ToString() | ConvertFrom-Json
                if ($candidate.id -eq $id) { $response = $builder.ToString(); break }
            } catch {
                # fragment incomplet : continuer a recevoir
            }
        }
        if (-not $response) { throw 'Pas de reponse DevTools.' }
        $parsed = $response | ConvertFrom-Json
        if ($parsed.error) { throw ('DevTools: ' + $parsed.error.message) }
        return $parsed.result
    } finally {
        $ws.Dispose()
    }
}

# Masque le signal d'automatisation dans les pages futures : Chrome en mode
# debug pur (sans --enable-automation) n'a en realite qu'un seul signal
# visible, navigator.webdriver ; on le neutralise en amont de chaque
# navigation. Le reste (plugins, WebGL, en-tetes, UA) est deja celui d'un
# vrai Chrome fenetre, contrairement a chromedriver/headless.
function Invoke-CdpStealth([string]$wsUrl) {
    $source = "Object.defineProperty(navigator, 'webdriver', { get: () => undefined });"
    [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Page.addScriptToEvaluateOnNewDocument' -Params @{ source = $source })
}

# Sequence de touches clavier CDP (meme nommage que les touches sures cote Node).
function Invoke-CdpKey([string]$wsUrl, [string]$key) {
    $mapping = @{
        Enter     = @{ vk = 13;  code = 'Enter';     key = 'Enter' }
        Tab       = @{ vk = 9;   code = 'Tab';       key = 'Tab' }
        Space     = @{ vk = 32;  code = 'Space';     key = ' ' }
        Escape    = @{ vk = 27;  code = 'Escape';    key = 'Escape' }
        Backspace = @{ vk = 8;   code = 'Backspace'; key = 'Backspace' }
        Delete    = @{ vk = 46;  code = 'Delete';    key = 'Delete' }
        Home      = @{ vk = 36;  code = 'Home';      key = 'Home' }
        End       = @{ vk = 35;  code = 'End';       key = 'End' }
        ArrowUp    = @{ vk = 38; code = 'ArrowUp';    key = 'ArrowUp' }
        ArrowDown  = @{ vk = 40; code = 'ArrowDown';  key = 'ArrowDown' }
        ArrowLeft  = @{ vk = 37; code = 'ArrowLeft';  key = 'ArrowLeft' }
        ArrowRight = @{ vk = 39; code = 'ArrowRight'; key = 'ArrowRight' }
        PageUp    = @{ vk = 33; code = 'PageUp';    key = 'PageUp' }
        PageDown  = @{ vk = 34; code = 'PageDown';  key = 'PageDown' }
        F6        = @{ vk = 117; code = 'F6';        key = 'F6' }
        'Ctrl+L'  = @{ vk = 76;  code = 'KeyL';      key = 'l'; mod = 2 }
    }
    $m = $mapping[$key]
    if (-not $m) { throw ('Touche navigateur non prise en charge : ' + $key) }
    if ($m.mod) {
        [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Input.dispatchKeyEvent' -Params @{ type = 'rawKeyDown'; modifiers = $m.mod; key = $m.key; code = $m.code; windowsVirtualKeyCode = $m.vk })
        [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Input.dispatchKeyEvent' -Params @{ type = 'char'; modifiers = $m.mod; key = $m.key; code = $m.code; text = $m.key })
        [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Input.dispatchKeyEvent' -Params @{ type = 'keyUp'; modifiers = $m.mod; key = $m.key; code = $m.code; windowsVirtualKeyCode = $m.vk })
    } else {
        [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Input.dispatchKeyEvent' -Params @{ type = 'keyDown'; key = $m.key; code = $m.code; windowsVirtualKeyCode = $m.vk })
        [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Input.dispatchKeyEvent' -Params @{ type = 'keyUp'; key = $m.key; code = $m.code; windowsVirtualKeyCode = $m.vk })
    }
}

function Capture-Screen {
    $left = [ScreenAgent]::VirtualLeft()
    $top = [ScreenAgent]::VirtualTop()
    $width = [ScreenAgent]::VirtualWidth()
    $height = [ScreenAgent]::VirtualHeight()
    if ($width -le 0 -or $height -le 0) { throw 'Aucun ecran disponible sur ce poste.' }
    $bitmap = New-Object Drawing.Bitmap($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($left, $top, 0, 0, (New-Object Drawing.Size($width, $height)))
        $cursorInfo = New-Object ScreenAgent+CURSORINFO
        $cursorInfo.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($cursorInfo)
        if ([ScreenAgent]::GetCursorInfo([ref]$cursorInfo) -and $cursorInfo.hCursor -ne [IntPtr]::Zero) {
            $hdc = $graphics.GetHdc()
            try {
                [ScreenAgent]::DrawIcon($hdc, ($cursorInfo.ptScreenPos.X - $left), ($cursorInfo.ptScreenPos.Y - $top), $cursorInfo.hCursor) | Out-Null
            } finally {
                $graphics.ReleaseHdc($hdc)
            }
        }
        return Convert-Bitmap -sourceBitmap $bitmap -sourceWidth $width -sourceHeight $height
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Capture-Window([long]$targetWindow) {
    $rectText = [ScreenAgent]::WindowRectText($targetWindow)
    $parts = $rectText -split '\|'
    $left = [int]$parts[0]; $top = [int]$parts[1]; $right = [int]$parts[2]; $bottom = [int]$parts[3]
    $width = $right - $left
    $height = $bottom - $top
    if ($width -le 0 -or $height -le 0) { throw 'La fenetre cible est invalide ou reduite.' }
    $bitmap = New-Object Drawing.Bitmap($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $hdc = $graphics.GetHdc()
        try {
            [ScreenAgent]::PrintWindowEx($targetWindow, $hdc) | Out-Null
        } finally {
            $graphics.ReleaseHdc($hdc)
        }
        return Convert-Bitmap -sourceBitmap $bitmap -sourceWidth $width -sourceHeight $height
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

# UI Automation : champs editables (ValuePattern) d'une fenetre, sans focus.
# L'activation WM_GETOBJECT est envoyee au top-level et a ses enfants Chromium
# pour que le renderer construise l'arbre (Chrome/Edge/WebView2 l'exigent).
function Get-UiaValueElements([long]$hwnd) {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    [ScreenAgent]::SendGetObject($hwnd)
    foreach ($childLine in [ScreenAgent]::ChromiumChildHandles($hwnd)) {
        $parts = $childLine -split [string][char]0x1F
        [ScreenAgent]::SendGetObject([long]$parts[0])
    }
    Start-Sleep -Milliseconds 700
    $root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$hwnd)
    $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $pairs = @()
    foreach ($e in $all) {
        $vp = $null
        try { $vp = $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern) } catch { continue }
        $name = ''; $ctype = ''; $value = ''
        try { $name = [string]$e.Current.Name } catch { }
        try { $ctype = [string]$e.Current.ControlType.ProgrammaticName } catch { }
        try { $value = [string]$vp.Current.Value } catch { }
        $pairs += @{ element = $e; pattern = $vp; name = $name; type = $ctype; value = $value }
        if ($pairs.Count -ge 30) { break }
    }
    # La virgule unaire evite le deroulement du tableau au retour.
    , $pairs
}

# OCR Windows natif : retourne les lignes texte avec leur rectangle (dans
# l'espace de coordonnees du bitmap fourni, sans rescale). Le bitmap GDI+ est
# exporte en PNG temporaire : RecognizeAsync exige un SoftwareBitmap WinRT et
# le chemin fichier -> BitmapDecoder est le pont fiable entre les deux mondes.
function Find-OcrCandidates($bitmap, [string]$needle) {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
    $null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType = WindowsRuntime]
    $null = [Windows.Storage.StorageFile, Windows.Foundation, ContentType = WindowsRuntime]
    $genericName = 'IAsyncOperation' + [char]96 + '1'
    $awaitGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq $genericName })[0]
    if ($null -eq $awaitGeneric) { throw 'Runtime Windows indisponible pour l''OCR.' }
    $recognize = $awaitGeneric.MakeGenericMethod([Windows.Media.Ocr.OcrResult])
    $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    if ($null -eq $engine) { $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage([Windows.Globalization.Language]::new('fr-FR')) }
    if ($null -eq $engine) { $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage([Windows.Globalization.Language]::new('en-US')) }
    if ($null -eq $engine) { throw 'Aucun moteur OCR disponible sur ce poste.' }
    $tempPng = Join-Path $env:TEMP ('cst-locate-' + [guid]::NewGuid().ToString('N') + '.png')
    $bitmap.Save($tempPng, [Drawing.Imaging.ImageFormat]::Png)
    try {
        $awaitOp = { param($winRtOperation, $resultType) $awaitGeneric.MakeGenericMethod($resultType).Invoke($null, @($winRtOperation)).Result }
        $file = & $awaitOp ([Windows.Storage.StorageFile]::GetFileFromPathAsync($tempPng)) ([Windows.Storage.StorageFile])
        $stream = & $awaitOp ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
        $decoder = & $awaitOp ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
        $software = & $awaitOp ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
        $task = $recognize.Invoke($null, @($engine.RecognizeAsync($software)))
        $task.Wait(-1) | Out-Null
        $result = $task.Result
    } finally {
        Remove-Item $tempPng -ErrorAction SilentlyContinue
    }
    $needleLower = $needle.ToLowerInvariant()
    $candidates = @()
    foreach ($line in $result.Lines) {
        if ($line.Text.ToLowerInvariant().Contains($needleLower)) {
            $minX = [int]::MaxValue; $minY = [int]::MaxValue; $maxX = 0; $maxY = 0
            foreach ($word in $line.Words) {
                $r = $word.BoundingRect
                if ([int]$r.X -lt $minX) { $minX = [int]$r.X }
                if ([int]$r.Y -lt $minY) { $minY = [int]$r.Y }
                if ([int]($r.X + $r.Width) -gt $maxX) { $maxX = [int]($r.X + $r.Width) }
                if ([int]($r.Y + $r.Height) -gt $maxY) { $maxY = [int]($r.Y + $r.Height) }
            }
            $candidates += @{
                text = $line.Text
                x = $minX; y = $minY; w = ($maxX - $minX); h = ($maxY - $minY)
                clickX = [int](($minX + $maxX) / 2)
                clickY = [int](($minY + $maxY) / 2)
            }
        }
    }
    # La virgule unaire empêche PowerShell de dérouler le tableau au retour.
    , $candidates
}

try {
    $action = [string]$request.action
    switch ($action) {
        'health' {
            Write-Result @{ ok = $true; ready = $true }
            break
        }
        'uia_fields' {
            $hwnd = [long]$request.window
            $pairs = Get-UiaValueElements $hwnd
            $fields = @()
            $i = 0
            foreach ($p in $pairs) {
                $fields += @{ index = $i; name = $p.name; type = $p.type; value = $p.value }
                $i++
            }
            $selected = $null
            if ($request.PSObject.Properties.Name -contains 'index' -and $null -ne $request.index) {
                if ($fields.Count -eq 0) { throw 'Aucun champ editable UIA dans la fenetre cible.' }
                if ([int]$request.index -ge $fields.Count) { throw 'Index de champ hors limite.' }
                $selected = $fields[[int]$request.index]
            }
            Write-Result @{
                ok = $true
                count = $fields.Count
                fields = @($fields)
                selected = $selected
                window = $hwnd
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'locate' {
            $needle = [string]$request.text
            $candidates = @()
            $space = 'screen'
            if ($request.window) {
                # Fenetre cible : PrintWindow a taille reelle, coordonnees CLIENT
                # directement utilisables par click avec le meme window.
                $space = 'client'
                $windowId = [long]$request.window
                $rectText = [ScreenAgent]::WindowRectText($windowId)
                $parts = $rectText -split '\|'
                $width = [int]$parts[2] - [int]$parts[0]
                $height = [int]$parts[3] - [int]$parts[1]
                if ($width -le 0 -or $height -le 0) { throw 'La fenetre cible est invalide ou reduite.' }
                $bitmap = New-Object Drawing.Bitmap($width, $height)
                $graphics = [Drawing.Graphics]::FromImage($bitmap)
                try {
                    $hdc = $graphics.GetHdc()
                    try { [ScreenAgent]::PrintWindowEx($windowId, $hdc) | Out-Null } finally { $graphics.ReleaseHdc($hdc) }
                    $candidates = Find-OcrCandidates -bitmap $bitmap -needle $needle
                } finally { $graphics.Dispose(); $bitmap.Dispose() }
            } else {
                # Ecran entier : bitmap natif sans rescale, coordonnees converties
                # en espace ecran (SetCursorPos) pour un clic direct.
                $left = [ScreenAgent]::VirtualLeft(); $top = [ScreenAgent]::VirtualTop()
                $width = [ScreenAgent]::VirtualWidth(); $height = [ScreenAgent]::VirtualHeight()
                if ($width -le 0 -or $height -le 0) { throw 'Aucun ecran disponible sur ce poste.' }
                $bitmap = New-Object Drawing.Bitmap($width, $height)
                $graphics = [Drawing.Graphics]::FromImage($bitmap)
                try {
                    $graphics.CopyFromScreen($left, $top, 0, 0, (New-Object Drawing.Size($width, $height)))
                    $candidates = Find-OcrCandidates -bitmap $bitmap -needle $needle
                    for ($i = 0; $i -lt $candidates.Count; $i++) {
                        $candidates[$i].clickX += $left
                        $candidates[$i].clickY += $top
                    }
                } finally { $graphics.Dispose(); $bitmap.Dispose() }
            }
            $selected = $null
            if ($request.PSObject.Properties.Name -contains 'index' -and $null -ne $request.index) {
                if ($candidates.Count -eq 0) { throw 'Aucun candidat a selectionner.' }
                if ([int]$request.index -ge $candidates.Count) { throw 'Index de candidat hors limite.' }
                $selected = $candidates[[int]$request.index]
            }
            Write-Result @{
                ok = $true
                space = $space
                count = $candidates.Count
                candidates = @($candidates)
                selected = $selected
                window = $request.window
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'screenshot' {
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'move' {
            $x = [int]$request.x
            $y = [int]$request.y
            [ScreenAgent]::Move($x, $y) | Out-Null
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'click' {
            $x = [int]$request.x
            $y = [int]$request.y
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            if ($target) {
                [ScreenAgent]::ClickWindowClient($target, $x, $y, $false, $false)
                $capture = Capture-Window $target
            } else {
                [ScreenAgent]::Click($x, $y, $false, $false)
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'click'
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'double_click' {
            $x = [int]$request.x
            $y = [int]$request.y
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            if ($target) {
                [ScreenAgent]::ClickWindowClient($target, $x, $y, $false, $true)
                $capture = Capture-Window $target
            } else {
                [ScreenAgent]::Click($x, $y, $false, $true)
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'double_click'
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'right_click' {
            $x = [int]$request.x
            $y = [int]$request.y
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            if ($target) {
                [ScreenAgent]::ClickWindowClient($target, $x, $y, $true, $false)
                $capture = Capture-Window $target
            } else {
                [ScreenAgent]::Click($x, $y, $true, $false)
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'right_click'
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'type' {
            $text = [string]$request.text
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            $mode = if ($request.PSObject.Properties.Name -contains 'mode') { [string]$request.mode } else { 'messages' }
            if ($target -and $mode -eq 'uia') {
                # Saisie par UI Automation ValuePattern : zero focus, la valeur
                # du champ est posee par le provider de l'app elle-meme.
                $pairs = Get-UiaValueElements $target
                if ($pairs.Count -eq 0) { throw 'Aucun champ editable UIA dans la fenetre cible (arbre indisponible ou vide).' }
                $idx = 0
                if ($request.PSObject.Properties.Name -contains 'uiaIndex' -and $null -ne $request.uiaIndex) { $idx = [int]$request.uiaIndex }
                if ($idx -ge $pairs.Count) { throw ('Index UIA hors limite ({0} champs).' -f $pairs.Count) }
                $picked = $pairs[$idx]
                $picked.pattern.SetValue($text)
                # Chromium propage la valeur UIA de facon asynchrone ET sert
                # des valeurs stale sur un element longuement detenu : la
                # relecture re-walke l'arbre (elements neufs) avec retries.
                $after = ''
                $verified = $false
                for ($try2 = 0; $try2 -lt 5; $try2++) {
                    Start-Sleep -Milliseconds 400
                    try {
                        $freshPairs = Get-UiaValueElements $target
                        if ($idx -lt $freshPairs.Count) {
                            $after = [string]$freshPairs[$idx].pattern.Current.Value
                        }
                    } catch { $after = '' }
                    if ($after -eq $text) { $verified = $true; break }
                }
                Write-Result @{
                    ok = $true
                    typed = $text.Length
                    mode = 'uia'
                    window = $target
                    uiaIndex = $idx
                    fieldName = $picked.name
                    fieldValue = $after
                    verified = $verified
                    activeWindow = ([ScreenAgent]::ForegroundTitle())
                }
                break
            }
            if ($target) {
                if ($mode -eq 'focus') { [ScreenAgent]::TypeFocusSteal($target, $text) } else { [ScreenAgent]::TypeToWindow($target, $text) }
                $capture = Capture-Window $target
            } else {
                [ScreenAgent]::Type($text)
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                typed = $text.Length
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'press' {
            $key = [string]$request.key
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            $mode = if ($request.PSObject.Properties.Name -contains 'mode') { [string]$request.mode } else { 'messages' }
            if ($target) {
                if ($mode -eq 'focus') { [ScreenAgent]::PressFocusSteal($target, [ScreenAgent]::GetVkSequence($key)) } else { [ScreenAgent]::PressToWindow($target, [ScreenAgent]::GetVkSequence($key)) }
                $capture = Capture-Window $target
            } else {
                [ScreenAgent]::PressSequence([ScreenAgent]::GetVkSequence($key))
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                key = $key
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'scroll' {
            $amount = [int]$request.amount
            $windowMode = $request.PSObject.Properties.Name -contains 'window'
            $target = if ($windowMode) { [long]$request.window } else { $null }
            if ($target) {
                $sx = if ($request.PSObject.Properties.Name -contains 'x') { [int]$request.x } else { -1 }
                $sy = if ($request.PSObject.Properties.Name -contains 'y') { [int]$request.y } else { -1 }
                [ScreenAgent]::ScrollWindowClient($target, $amount, $sx, $sy)
                $capture = Capture-Window $target
            } else {
                if ($request.PSObject.Properties.Name -contains 'x') {
                    [ScreenAgent]::Move([int]$request.x, [int]$request.y) | Out-Null
                }
                [ScreenAgent]::Scroll($amount)
                $capture = Capture-Screen
            }
            Write-Result @{
                ok = $true
                amount = $amount
                window = $target
                screenshot = $capture.base64
                width = $capture.width
                height = $capture.height
                screenWidth = $capture.screenWidth
                screenHeight = $capture.screenHeight
                cursorX = [ScreenAgent]::CursorX()
                cursorY = [ScreenAgent]::CursorY()
                activeWindow = ([ScreenAgent]::ForegroundTitle())
            }
            break
        }
        'windows' {
            $raw = @([ScreenAgent]::ListWindowsText())
            $windows = @()
            foreach ($line in $raw) {
                $parts = $line -split [string][char]0x1F
                if ($parts.Count -ge 7) {
                    $windows += [pscustomobject]@{
                        id = [long]$parts[0]
                        pid = [int]$parts[1]
                        title = $parts[2]
                        left = [int]$parts[3]
                        top = [int]$parts[4]
                        right = [int]$parts[5]
                        bottom = [int]$parts[6]
                    }
                }
            }
            $windows = @($windows | Select-Object -First 60)
            Write-Result @{ ok = $true; count = $windows.Count; windows = $windows }
            break
        }
        'open' {
            $command = [string]$request.command
            $firstWord = (($command.TrimStart() -split '\s+')[0]).ToLowerInvariant()
            $isBrowser = $firstWord -in @('chrome', 'chrome.exe', 'msedge', 'msedge.exe')
            $debugPort = $null
            $profile = $null
            if ($isBrowser) {
                # Profil isole (pas de comptes/mots de passe de l'utilisateur)
                # + port de debug : indispensable pour piloter la page en
                # arriere-plan sans voler le focus (le forwarding
                # single-instance de Chrome prendrait le focus).
                if ($command -notmatch '--user-data-dir=') {
                    $profile = Join-Path $env:TEMP ('cst-screen-chrome-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
                    $command = $command + ' --user-data-dir="' + $profile + '" --no-first-run'
                } elseif ($command -match '--user-data-dir="([^"]+)"') {
                    $profile = $Matches[1]
                } elseif ($command -match '--user-data-dir=([^\s]+)') {
                    $profile = $Matches[1]
                }
                if ($command -notmatch '--remote-debugging-port=') {
                    $command = $command + ' --remote-debugging-port=0'
                }
                if ($command -notmatch '--remote-allow-origins=') {
                    # Chrome recent refuse les connexions DevTools dont l'origine
                    # n'est pas autorisee (403 sur le handshake WebSocket).
                    $command = $command + ' --remote-allow-origins=*'
                }
                if ($command -notmatch '--start-minimized') {
                    # Chrome active sa premiere fenetre malgre SW_SHOWNOACTIVATE :
                    # la demarrer reduite garantit qu'aucun focus n'est vole
                    # (l'agent pilote la page par CDP, pas besoin de la voir).
                    $command = $command + ' --start-minimized'
                }
                if ($command -notmatch '--disable-blink-features=') {
                    # Supprime le marqueur d'automatisation navigator.webdriver
                    # (Google et d'autres sites verifient ce signal).
                    $command = $command + ' --disable-blink-features=AutomationControlled'
                }
                if ($command -notmatch '--force-renderer-accessibility') {
                    # Construit l'arbre UIA du CONTENU web (champs de page
                    # editables via ValuePattern) des le demarrage.
                    $command = $command + ' --force-renderer-accessibility'
                }
                if ($command -notmatch '--disable-features=') {
                    # CalculateNativeWinOcclusion : Windows marque les fenetres
                    # d'arriere-plan comme occluses et Chrome SUSPEND leur
                    # renderer — l'arbre UIA du contenu web n'existe alors
                    # jamais (et les valeurs lues restent une generation de
                    # retard). Desactiver l'occlusion garde le renderer vivant
                    # en arriere-plan : UIA voit le document et ses champs.
                    $command = $command + ' --disable-features=CalculateNativeWinOcclusion'
                }
            }
            $hidden = $false
            if ($request.PSObject.Properties.Name -contains 'hidden') { $hidden = [bool]$request.hidden }
            $started = [ScreenAgent]::LaunchNoFocus($command, $hidden)
            if (-not $started) { throw 'Le lancement sans focus a echoue.' }
            if ($isBrowser -and $profile) {
                $portFile = Join-Path $profile 'DevToolsActivePort'
                $deadline = [DateTime]::UtcNow.AddSeconds(20)
                while ([DateTime]::UtcNow -lt $deadline -and -not (Test-Path $portFile)) { Start-Sleep -Milliseconds 300 }
                if (Test-Path $portFile) {
                    $portLine = ((Get-Content $portFile -TotalCount 1) | Select-Object -First 1).Trim()
                    if ($portLine -match '^\d+$') { $debugPort = [int]$portLine }
                    # Neutraliser navigator.webdriver des maintenant : les
                    # navigations suivantes (et rechargements) de l'onglet
                    # lance sont deja masquees.
                    try {
                        $page = Get-CdpPageTarget -Port $debugPort
                        Invoke-CdpStealth -WsUrl $page.webSocketDebuggerUrl
                    } catch { }
                }
            }
            Write-Result @{ ok = $true; started = $true; command = $command; debugPort = $debugPort }
            break
        }
        'browser' {
            $method = [string]$request.method
            if (-not $method) { throw 'Methode navigateur manquante.' }
            # Port CDP explicite (WebView2 de l'app via CST_WEBVIEW_CDP_PORT)
            # sinon resolution par la fenetre du navigateur lance.
            if ($request.PSObject.Properties.Name -contains 'cdpPort' -and $request.cdpPort) {
                $port = [int]$request.cdpPort
            } else {
                $port = Get-CdpPort ([long]$request.window)
            }
            if ($method -eq 'list') {
                $targets = Invoke-RestMethod -Uri ('http://127.0.0.1:{0}/json/list' -f $port) -TimeoutSec 10
                $pages = @($targets | Where-Object { $_.type -eq 'page' } | Select-Object -First 20 | ForEach-Object {
                    @{ id = $_.id; title = $_.title; url = $_.url }
                })
                Write-Result @{ ok = $true; count = $pages.Count; targets = $pages }
                break
            }
            $page = Get-CdpPageTarget -Port $port
            $wsUrl = [string]$page.webSocketDebuggerUrl
            switch ($method) {
                'screenshot' {
                    $result = Invoke-Cdp -WsUrl $wsUrl -Method 'Page.captureScreenshot' -Params @{ format = 'jpeg'; quality = 72 }
                    Write-Result @{ ok = $true; pageCapture = $true; window = $request.window; screenshot = $result.data; title = $page.title; url = $page.url }
                    break
                }
                'navigate' {
                    $url = [string]$request.url
                    Invoke-CdpStealth -WsUrl $wsUrl
                    $result = Invoke-Cdp -WsUrl $wsUrl -Method 'Page.navigate' -Params @{ url = $url }
                    # Consentement Google sur profil neuf : cliquer « Tout
                    # accepter » (ID stable L2AGLb) comme le ferait un
                    # utilisateur. Sans ces cookies, Google repond par sa page
                    # anti-bot (« trafic exceptionnel ») — jamais sur le vrai
                    # profil de l'utilisateur, uniquement le profil isole.
                    Start-Sleep -Seconds 3
                    try {
                        $check = Invoke-Cdp -WsUrl $wsUrl -Method 'Runtime.evaluate' -Params @{ expression = "document.querySelector('#L2AGLb') !== null"; returnByValue = $true }
                        if ($check.result.value -eq $true) {
                            [void](Invoke-Cdp -WsUrl $wsUrl -Method 'Runtime.evaluate' -Params @{ expression = "(() => { const b = document.querySelector('#L2AGLb'); if (b) { b.click(); return 'OK'; } return 'ABSENT'; })()"; returnByValue = $true })
                            Start-Sleep -Seconds 2
                        }
                    } catch { }
                    Write-Result @{ ok = $true; navigated = $true; url = $url; frameId = $result.frameId }
                    break
                }
                'eval' {
                    $expression = [string]$request.expression
                    $result = Invoke-Cdp -WsUrl $wsUrl -Method 'Runtime.evaluate' -Params @{ expression = $expression; returnByValue = $true; awaitPromise = $true }
                    if ($result.exceptionDetails) {
                        throw ('Erreur JavaScript dans la page : ' + $result.exceptionDetails.text)
                    }
                    $valueJson = $null
                    if ($null -ne $result.result.value) { $valueJson = ($result.result.value | ConvertTo-Json -Depth 6 -Compress) }
                    Write-Result @{ ok = $true; value = $valueJson; url = $page.url }
                    break
                }
                'click' {
                    $selector = [string]$request.selector
                    $js = "(() => { const el = document.querySelector($(ConvertTo-Json $selector -Compress)); if (!el) return 'NOT_FOUND'; el.scrollIntoView({ block: 'center' }); ['pointerdown','mousedown','pointerup','mouseup','click'].forEach(t => el.dispatchEvent(new MouseEvent(t, { bubbles: true, cancelable: true, view: window }))); el.click(); return 'OK'; })()"
                    $result = Invoke-Cdp -WsUrl $wsUrl -Method 'Runtime.evaluate' -Params @{ expression = $js; returnByValue = $true; awaitPromise = $true }
                    if ($result.exceptionDetails) { throw ('Erreur JavaScript dans la page : ' + $result.exceptionDetails.text) }
                    if ($result.result.value -eq 'NOT_FOUND') { throw ('Element introuvable avec le selecteur : ' + $selector) }
                    Write-Result @{ ok = $true; clicked = $true; selector = $selector; url = $page.url }
                    break
                }
                'type' {
                    $selector = [string]$request.selector
                    $text = [string]$request.text
                    $js = "(() => { const el = document.querySelector($(ConvertTo-Json $selector -Compress)); if (!el) return 'NOT_FOUND'; el.focus(); const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; const setter = Object.getOwnPropertyDescriptor(proto, 'value').set; setter.call(el, $(ConvertTo-Json $text -Compress)); el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return 'OK'; })()"
                    $result = Invoke-Cdp -WsUrl $wsUrl -Method 'Runtime.evaluate' -Params @{ expression = $js; returnByValue = $true; awaitPromise = $true }
                    if ($result.exceptionDetails) { throw ('Erreur JavaScript dans la page : ' + $result.exceptionDetails.text) }
                    if ($result.result.value -eq 'NOT_FOUND') { throw ('Element introuvable avec le selecteur : ' + $selector) }
                    Write-Result @{ ok = $true; typed = $true; selector = $selector; url = $page.url }
                    break
                }
                'key' {
                    $key = [string]$request.key
                    Invoke-CdpKey -WsUrl $wsUrl -Key $key
                    Write-Result @{ ok = $true; key = $key; url = $page.url }
                    break
                }
                default {
                    throw 'Methode navigateur non prise en charge.'
                }
            }
            break
        }
        default {
            throw 'Action ecran non prise en charge.'
        }
    }
    } catch {
        $ex = $_.Exception
        while ($ex -is [System.AggregateException] -and $ex.InnerException) { $ex = $ex.InnerException }
        $message = $ex.Message
        Write-Result @{ ok = $false; error = $message }
    }
}
`;

// Injecte le mapping touche -> sequence de codes virtuels dans le C# compile :
// evite de passer un dictionnaire PowerShell et garde la validation cote Node
// comme source unique des touches autorisees.
export const buildAgentScreenPowerShellScriptWithKeys = () => {
  const script = buildAgentScreenPowerShellScript();
  const marker = "        // SCREEN_KEY_VK_MAPPING";
  if (!script.includes(marker)) {
    throw new Error("Marqueur GetVkSequence absent du script PowerShell.");
  }
  const mapping = Object.entries(SCREEN_KEY_VK)
    .map(([key, vks]) => `        if (key == "${key}") return new int[] { ${vks.map((vk) => `0x${vk.toString(16)}`).join(", ")} };`)
    .join("\n");
  return script.replace(marker, mapping);
};

// Le script PowerShell (bloc C# inclus) depasse la limite de ligne de commande
// Windows : il est ecrit une fois dans un fichier temporaire au nom hache et
// lance avec -File. La requete reste portee par stdin, la reponse par stdout.
const powershellCommandFor = (scriptFile) => [
  "-NoProfile",
  "-STA",
  "-NonInteractive",
  "-ExecutionPolicy",
  "Bypass",
  "-File",
  scriptFile,
];

export class AgentScreenController {
  constructor({
    importPowershell = () => Promise.resolve("powershell.exe"),
    confirmAction = confirmWindowsScreenAction,
    maxSessions = 24,
    idleMilliseconds = 30 * 60 * 1000,
    spawnImpl = spawn,
    responseCap = MAX_AGENT_SCREEN_RESPONSE_BYTES,
    nowImpl = Date.now,
    executeImpl = null,
    requestTimeoutMilliseconds = 90_000,
  } = {}) {
    this.importPowershell = importPowershell;
    this.confirmAction = confirmAction;
    this.maxSessions = maxSessions;
    this.idleMilliseconds = idleMilliseconds;
    this.spawnImpl = spawnImpl;
    this.responseCap = responseCap;
    this.now = nowImpl;
    this.executeImpl = executeImpl;
    this.requestTimeoutMilliseconds = requestTimeoutMilliseconds;
    this.powershellPath = null;
    this.scriptFile = null;
    this.sessions = new Map();
    this.queues = new Map();
    this.script = buildAgentScreenPowerShellScriptWithKeys();
    // Relais persistant : un seul processus PowerShell sert toutes les requetes
    // (compilation C# unique, plus de demarrage par action). Les requetes sont
    // serialisees sur ce relais, puisqu'elles partagent le meme stdin/stdout.
    this.relay = null;
    this.relayExitCode = null;
    this.relayQueue = Promise.resolve();
    this.cleanupTimer = setInterval(() => this.pruneIdleSessions(), 60_000);
    this.cleanupTimer.unref?.();
  }

  async powershell() {
    if (!this.powershellPath) {
      this.powershellPath = await this.importPowershell();
    }
    return this.powershellPath;
  }

  async scriptPath() {
    if (!this.scriptFile) {
      const digest = createHash("sha256").update(this.script, "utf8").digest("hex").slice(0, 12);
      const path = join(tmpdir(), `cst-agent-screen-${digest}.ps1`);
      if (!existsSync(path)) {
        // BOM UTF-8 : PowerShell 5.1 lit ainsi les accents eventuels.
        writeFileSync(path, `\ufeff${this.script}`, "utf8");
      }
      this.scriptFile = path;
    }
    return this.scriptFile;
  }

  // Lance le relais PowerShell persistant s'il n'est pas deja vivant. Le
  // demarrage couteux (powershell.exe + compilation du bloc C#) n'a lieu
  // qu'ici, une seule fois pour toute la vie du controller.
  async ensureRelay() {
    if (this.relay && this.relayExitCode === null) return this.relay;
    const powershellPath = await this.powershell();
    const scriptFile = await this.scriptPath();
    const child = this.spawnImpl(powershellPath, powershellCommandFor(scriptFile), {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.relay = child;
    this.relayExitCode = null;
    let settled = false;
    const markDead = (code) => {
      if (settled) return;
      settled = true;
      this.relayExitCode = code;
    };
    child.once("error", () => markDead(1));
    child.once("close", () => markDead(0));
    return child;
  }

  // Pre-chauffe : demarre le relais en arriere-plan (broker demarre) pour que
  // la premiere action de l'agent ne paie pas le demarrage PowerShell + la
  // compilation du script. Inactif quand un executeur de test est injecte.
  async warmUp() {
    if (this.executeImpl) return;
    await this.ensureRelay().catch(() => undefined);
  }

  async execute(request) {
    await this.ensureRelay();
    const payload = Buffer.from(JSON.stringify(request), "utf8").toString("base64");
    // Les requetes partagent le meme relais (stdin/stdout) : elles sont
    // serialisees, et chaque echec laisse la place a la suivante.
    const previous = this.relayQueue;
    const next = previous.catch(() => undefined).then(() => this.relayRoundTrip(payload));
    this.relayQueue = next.catch(() => undefined);
    return next;
  }

  // Envoie une requete au relais persistant et attend sa reponse sur une
  // seule ligne de stdout (Write-Result termine chaque reponse par \n + flush).
  relayRoundTrip(payload) {
    return new Promise((resolveExecute, rejectExecute) => {
      const child = this.relay;
      if (!child || this.relayExitCode !== null) {
        rejectExecute(new Error("Le relais PowerShell du poste Windows est indisponible."));
        return;
      }
      let buffer = "";
      let settled = false;
      const cleanup = () => {
        clearTimeout(timer);
        child.stdout.removeListener("data", onData);
        child.removeListener("close", onClose);
        child.removeListener("error", onError);
      };
      const finishError = (message) => {
        if (settled) return;
        settled = true;
        cleanup();
        // L'etat du relais est inconnu apres un blocage : on le tue, la
        // prochaine requete en demarrera un neuf.
        child.kill?.();
        rejectExecute(new Error(message));
      };
      const onData = (chunk) => {
        if (settled) return;
        buffer += chunk.toString("utf8");
        if (Buffer.byteLength(buffer, "utf8") > this.responseCap) {
          finishError("La capture d'ecran est trop volumineuse.");
          return;
        }
        const newline = buffer.indexOf("\n");
        if (newline === -1) return;
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        settled = true;
        cleanup();
        if (!line) {
          rejectExecute(new Error("Le poste Windows n'a pas repondu."));
          return;
        }
        try {
          resolveExecute(JSON.parse(line));
        } catch {
          rejectExecute(new Error("Le poste Windows a renvoye une reponse invalide."));
        }
      };
      const onClose = () => finishError("Le relais PowerShell du poste Windows s'est arrete.");
      const onError = () => finishError("Le relais PowerShell du poste Windows est indisponible.");
      const timer = setTimeout(
        () => finishError("Le poste Windows a mis trop de temps a repondre."),
        this.requestTimeoutMilliseconds
      );
      child.stdout.on("data", onData);
      child.once("close", onClose);
      child.once("error", onError);
      child.stdin.write(`${payload}\n`);
    });
  }

  async pruneIdleSessions() {
    const cutoff = this.now() - this.idleMilliseconds;
    for (const [sessionId, session] of this.sessions) {
      if (session.lastAccess < cutoff) this.sessions.delete(sessionId);
    }
  }

  enqueue(sessionId, operation) {
    const previous = this.queues.get(sessionId) || Promise.resolve();
    const next = previous.catch(() => undefined).then(operation);
    this.queues.set(sessionId, next);
    void next.finally(() => {
      if (this.queues.get(sessionId) === next) this.queues.delete(sessionId);
    }).catch(() => undefined);
    return next;
  }

  async handle(rawRequest) {
    const request = validateAgentScreenRequest(rawRequest);
    return this.enqueue(request.sessionId, async () => {
      if (request.action === "health") {
        return { ok: true, ready: true };
      }
      if (this.sessions.size >= this.maxSessions && !this.sessions.has(request.sessionId)) {
        throw new Error("Trop de sessions ecran sont actives. Reessayez plus tard.");
      }
      const now = this.now();
      const previous = this.sessions.get(request.sessionId);
      const session = {
        lastAccess: now,
        armedUntil: previous?.armedUntil && previous.armedUntil > now ? previous.armedUntil : 0,
      };
      this.sessions.set(request.sessionId, session);

      // Desarmer ne restreint que cette session : aucune confirmation requise.
      if (request.action === "disarm") {
        session.armedUntil = 0;
        return { ok: true, armed: false };
      }

      // L'armement ne declenche pas de popup (affichage non fiable depuis un
      // processus en arriere-plan) : il n'est legitime qu'apres une demande
      // explicite de l'utilisateur dans le chat, et reste borne dans le temps.
      if (request.action === "arm") {
        session.armedUntil = now + request.minutes * 60_000;
        return {
          ok: true,
          armed: true,
          minutes: request.minutes,
          expiresAt: session.armedUntil,
          note: "Session armee : les actions passent sans confirmation jusqu'a expiration ou desarmement.",
        };
      }

      const armed = session.armedUntil > now;
      if (!armed) {
        const approved = await this.confirmAction({
          action: request.action === "browser" ? `browser:${request.method}` : request.action,
          x: request.x,
          y: request.y,
          key: request.key,
          text: request.text,
        });
        if (!approved) {
          throw new Error("Action annulee ou non confirmee sur le PC.");
        }
      }
      const result = await (this.executeImpl ? this.executeImpl(request) : this.execute(request));
      if (!result?.ok) {
        const message = String(result?.error || "Action ecran refusee par le poste Windows.");
        throw new Error(message);
      }
      return result;
    });
  }

  async close() {
    clearInterval(this.cleanupTimer);
    this.sessions.clear();
    if (this.relay) {
      try {
        this.relay.stdin?.end();
      } catch {
        // Le relais peut deja etre mort : rien d'autre a fermer.
      }
      this.relay.kill?.();
      this.relay = null;
      this.relayExitCode = null;
    }
  }
}
