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
  "move",
  "click",
  "double_click",
  "right_click",
  "type",
  "press",
  "scroll",
  "arm",
  "disarm",
]);

const MAX_SCREEN_COORDINATE = 100_000;
const MAX_TYPE_TEXT_LENGTH = 2000;
const MAX_SCROLL_NOTCHES = 20;
const DEFAULT_ARM_MINUTES = 10;
const MAX_ARM_MINUTES = 60;

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
    case "screenshot":
      break;
    case "move":
    case "click":
    case "double_click":
    case "right_click":
      request.x = asInteger(value.x, "Coordonnee X", -MAX_SCREEN_COORDINATE, MAX_SCREEN_COORDINATE);
      request.y = asInteger(value.y, "Coordonnee Y", -MAX_SCREEN_COORDINATE, MAX_SCREEN_COORDINATE);
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
      break;
    }
    case "press":
      request.key = asTrimmedString(value.key, "Touche", 32);
      if (!SAFE_SCREEN_KEYS.has(request.key)) {
        throw new Error("Cette touche n'est pas autorisee sur l'ecran.");
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
  if (action === "arm") return true;
  if (action === "click" || action === "double_click" || action === "right_click" || action === "type") {
    return true;
  }
  if (action === "press") return ["Enter", "Space"].includes(key);
  return false;
};

export const confirmWindowsScreenAction = ({ action, x, y, key = "", text = "", minutes = 0 }) =>
  new Promise((resolveApproval) => {
    if (!requiresScreenApproval(action, key)) {
      resolveApproval(true);
      return;
    }
    const details = [];
    if (action === "arm") {
      details.push(`Duree de la session armee : ${minutes} min`);
      details.push("Apres validation, les actions de cette session passent sans confirmation jusqu'a expiration.");
    }
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

    public static int[] GetVkSequence(string key)
    {
        // SCREEN_KEY_VK_MAPPING
        return null;
    }
}
'@
Add-Type -AssemblyName System.Drawing

$raw = [Console]::In.ReadToEnd()
if (-not $raw) { throw 'Requete ecran vide' }
$json = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(($raw.Trim())))
$request = $json | ConvertFrom-Json

function Write-Result([hashtable]$value) {
    $payload = $value | ConvertTo-Json -Compress -Depth 8
    [Console]::Out.Write($payload)
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
        $maxDimension = 1600
        $largest = [Math]::Max($width, $height)
        $scale = [Math]::Min(1.0, ($maxDimension / $largest))
        $targetWidth = [int][Math]::Max(1, [Math]::Round($width * $scale))
        $targetHeight = [int][Math]::Max(1, [Math]::Round($height * $scale))
        $scaled = New-Object Drawing.Bitmap($targetWidth, $targetHeight)
        $scaledGraphics = [Drawing.Graphics]::FromImage($scaled)
        try {
            $scaledGraphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $scaledGraphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
            $scaledGraphics.DrawImage($bitmap, 0, 0, $targetWidth, $targetHeight)
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
        return @{ base64 = $encoded; width = $targetWidth; height = $targetHeight; screenWidth = $width; screenHeight = $height }
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

try {
    $action = [string]$request.action
    switch ($action) {
        'health' {
            Write-Result @{ ok = $true; ready = $true }
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
            [ScreenAgent]::Click($x, $y, $false, $false)
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'click'
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
            [ScreenAgent]::Click($x, $y, $false, $true)
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'double_click'
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
            [ScreenAgent]::Click($x, $y, $true, $false)
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                x = $x
                y = $y
                action = 'right_click'
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
            [ScreenAgent]::Type($text)
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                typed = $text.Length
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
            [ScreenAgent]::PressSequence([ScreenAgent]::GetVkSequence($key))
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                key = $key
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
            if ($request.PSObject.Properties.Name -contains 'x') {
                [ScreenAgent]::Move([int]$request.x, [int]$request.y) | Out-Null
            }
            [ScreenAgent]::Scroll($amount)
            $capture = Capture-Screen
            Write-Result @{
                ok = $true
                amount = $amount
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
        default {
            throw 'Action ecran non prise en charge.'
        }
    }
} catch {
    $message = $_.Exception.Message
    Write-Result @{ ok = $false; error = $message }
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
  } = {}) {
    this.importPowershell = importPowershell;
    this.confirmAction = confirmAction;
    this.maxSessions = maxSessions;
    this.idleMilliseconds = idleMilliseconds;
    this.spawnImpl = spawnImpl;
    this.responseCap = responseCap;
    this.now = nowImpl;
    this.executeImpl = executeImpl;
    this.powershellPath = null;
    this.scriptFile = null;
    this.sessions = new Map();
    this.queues = new Map();
    this.script = buildAgentScreenPowerShellScriptWithKeys();
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

  async execute(request) {
    const powershellPath = await this.powershell();
    const scriptFile = await this.scriptPath();
    const payload = Buffer.from(JSON.stringify(request), "utf8").toString("base64");
    const child = this.spawnImpl(powershellPath, powershellCommandFor(scriptFile), {
      stdio: ["pipe", "pipe", "ignore"],
      windowsHide: true,
    });
    return new Promise((resolveExecute, rejectExecute) => {
      let stdout = "";
      let settled = false;
      const finishError = (message) => {
        if (settled) return;
        settled = true;
        child.kill?.();
        rejectExecute(new Error(message));
      };
      const timer = setTimeout(() => finishError("Le poste Windows a mis trop de temps a repondre."), 45_000);
      child.once("error", () => finishError("Le relais PowerShell du poste Windows est indisponible."));
      child.stdout.on("data", (chunk) => {
        if (settled) return;
        stdout += chunk.toString("utf8");
        if (Buffer.byteLength(stdout, "utf8") > this.responseCap) {
          finishError("La capture d'ecran est trop volumineuse.");
        }
      });
      child.once("close", () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (!stdout.trim()) {
          rejectExecute(new Error("Le poste Windows n'a pas repondu."));
          return;
        }
        try {
          resolveExecute(JSON.parse(stdout.trim()));
        } catch {
          rejectExecute(new Error("Le poste Windows a renvoye une reponse invalide."));
        }
      });
      child.stdin.end(`${payload}\n`);
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

      // L'armement est LA confirmation unique de la session : une seule popup
      // Windows locale, puis les actions passent sans popup jusqu'a expiration.
      if (request.action === "arm") {
        const approved = await this.confirmAction({
          action: "arm",
          minutes: request.minutes,
        });
        if (!approved) {
          throw new Error("Armement annule ou non confirme sur le PC.");
        }
        session.armedUntil = now + request.minutes * 60_000;
        return {
          ok: true,
          armed: true,
          minutes: request.minutes,
          expiresAt: session.armedUntil,
          note: "Les actions de cette session passent sans confirmation jusqu'a expiration ou desarmement.",
        };
      }

      const armed = session.armedUntil > now;
      if (!armed) {
        const approved = await this.confirmAction({
          action: request.action,
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
  }
}
