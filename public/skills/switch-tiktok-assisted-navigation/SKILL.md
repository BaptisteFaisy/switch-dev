---
name: switch-tiktok-assisted-navigation
description: Assist a user who explicitly asks to navigate TikTok manually on an Android device connected to Switch through the bounded USB-device CLI. Use for one-at-a-time remote-control actions and visual guidance; never use for unattended browsing, fake engagement, human impersonation, randomized behavior, or detection evasion.
---

# Switch TikTok Assisted Navigation

Keep the user in direct control of an Android device and a TikTok account they own or are authorized to use. This is assisted remote control, not a social-engagement bot.

## Preconditions

- Act only after an explicit request in the current conversation or a direct UI action from the user.
- Choose exactly one execution path: the Switch chat MCP tools or the bounded Freebuff terminal helper. Never submit the same action through both paths.
- List devices first. Use only an exact Android device ID returned by the current inventory; never guess a serial. If more than one eligible device is returned, require the user to select one and never choose the first automatically.
- Use only the structured actions documented below. Never enable raw shell access, call `adb` or `scrcpy` directly, alter the USB bridge safeguards, or bypass ownership, lease, approval, or confirmation checks.
- Do not guess the TikTok package name. Launch only an exact app target selected by the user or supplied by the device UI.

### Switch chat MCP path

1. Call `list_control_devices` before targeting an Android.
2. Call `control_device` with the exact shape `{ "deviceId": "...", "action": "...", "args": { ... }, "confirmed": true|false }`. Omit `args` when the action has none. Set `confirmed` to `true` for a state-changing action only after the user's explicit confirmation for that single action.
3. If the result is `queued` or `claimed`, call `get_control_device_action` with the returned `actionId` until it reaches a terminal state. Do not resubmit the action while waiting.

### Freebuff terminal path

- The terminal bridge provides `CST_DEVICE_API_URL`, `CST_DEVICE_TOKEN`, `CST_DEVICE_HELPER=cst-device`, and `CST_SERVER_BIN`. Require all four to be non-empty and check that the helper is callable through `PATH`. The parsed API URL must use HTTP(S), target exactly `localhost` or a loopback IP, use the exact path `/api/device-fleet`, and contain no username, password, query, or fragment. Never print the token, server-binary path, or complete environment. If a check fails, explain that the Switch USB bridge is not ready and stop.
- List the current inventory with `"$CST_DEVICE_HELPER" list` in a POSIX shell or `& $env:CST_DEVICE_HELPER list` in PowerShell.
- The only valid grammar is `cst-device list`, `cst-device action DEVICE_ID ACTION [ARGS_JSON] [--confirm] [--idempotency-key KEY]`, and `cst-device status ACTION_ID`. `ARGS_JSON`, when present, is one JSON object. The two action flags may surround that object, but each flag may appear only once.
- Before the first submission of each Freebuff action, create one fresh ASCII idempotency key, pass it with `--idempotency-key`, and retain it until the action reaches a terminal state. A manual retry after an ambiguous network failure must reuse the exact same key, device ID, action, JSON object, and confirmation flag. Never generate a new key for a retry. The helper generates a UUID and reuses it for its own bounded internal retries when the flag is omitted, but an explicit retained key is required whenever the command itself might be run again.
- If `action` returns `queued` or `claimed`, use the returned UUID with `status`. Never duplicate the action to work around a delay.

## Interaction model

One user action authorizes at most one matching state-changing input. Read-only inventory and screenshots do not consume that authorization. A request such as "scroll" may produce one swipe; it does not authorize a loop, queue, macro, timer, or background session.

1. Take a current screenshot and describe the visible state briefly.
2. State the single proposed action and its target when the screenshot is clear but the user's intended control is not. An unreadable, partial, stale, or visually ambiguous screenshot authorizes no input; ask the user to resolve it manually.
3. Execute only the requested action through the selected MCP or Freebuff path.
4. Take another screenshot when needed to verify the result. If the result is uncertain, stop instead of trying nearby coordinates.

The bridge supports exactly `info`, `screenshot`, `open_screen`, `tap`, `swipe`, `type_text`, `key_event`, `open_app`, and `shell`. Only `info` and `screenshot` are read-only. Every other action requires `confirmed: true` in MCP or `--confirm` in the Freebuff helper. Although `shell` exists in the bridge contract, this TikTok skill must never invoke it.

Freebuff examples follow the real helper contract. Each action line below is an independent example: set `IDEMPOTENCY_KEY` to a fresh UUID before its first execution, keep that value for an exact retry of that line only, and replace it before a different authorized action.

POSIX shell:

```sh
"$CST_DEVICE_HELPER" action "$DEVICE_ID" info --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" screenshot --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" open_screen --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" tap '{"x":120,"y":340}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" swipe '{"startX":120,"startY":700,"endX":120,"endY":240,"durationMs":350}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" type_text '{"text":"EXACT_TEXT"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" key_event '{"key":"HOME"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" action "$DEVICE_ID" open_app '{"appId":"EXACT_PACKAGE"}' --confirm --idempotency-key "$IDEMPOTENCY_KEY"
"$CST_DEVICE_HELPER" status "$ACTION_ID"
```

PowerShell uses the same action names and JSON object keys, for example:

```powershell
$idempotencyKey = [guid]::NewGuid().ToString()
& $env:CST_DEVICE_HELPER action $deviceId tap '{"x":120,"y":340}' --confirm --idempotency-key $idempotencyKey
& $env:CST_DEVICE_HELPER status $actionId
```

Do not fall back to another command when the bounded helper or an MCP tool rejects an action.

## Engagement boundary

- Never choose content to like, unlike, comment on, follow, share, save, message, purchase, upload, or publish autonomously.
- Require a separate, immediate confirmation before any engagement-changing action, even when the initial request says "like the current video." First take a fresh screenshot, describe the exact visible item without sensitive details, propose the one engagement action, and ask for confirmation. Bind confirmation to that visible state and invalidate it after any navigation, refresh, overlay, feed change, or other UI change. Earlier blanket permission, a session toggle, or a request to "act naturally" is not confirmation.
- Open comments or a profile only when the user explicitly requests that exact navigation. Never select profiles probabilistically or because they appear interesting.
- Never generate randomized delays, gesture paths, dwell times, watch patterns, or action ratios intended to resemble a human.
- Never claim the activity is human, undetectable, safe from enforcement, or compliant merely because a person started the session.
- Do not run unattended, across multiple accounts, or after the user leaves the session.

TikTok's [Terms of Service](https://www.tiktok.com/legal/page/row/terms-of-service/en) prohibit automated scripts interacting with the service, and its [Integrity and Authenticity rules](https://www.tiktok.com/community-guidelines/en/integrity-authenticity/) prohibit fake engagement and circumvention. Its [official API scopes](https://developers.tiktok.com/docs/en/tiktok-api-scopes) do not provide feed-scrolling, liking, commenting, or following controls. Keep real-service use equivalent to a manual remote-control action. Test any autonomous interaction engine only against a local mock UI, never TikTok.

## Stop conditions

Stop immediately when the user asks, the device disconnects, the selected device changes owner, or the UI shows a CAPTCHA, rate-limit, security challenge, age gate, payment prompt, account restriction, unexpected dialog, or uncertain target. Do not solve or bypass a challenge.

Keep only a minimal in-memory action journal containing timestamp, an opaque random alias created for the device in the current session, the user authorization, requested action, and success or failure. Never persist the journal or record account passwords, tokens, device IDs, raw serials, comment contents, or screenshot pixels. Do not upload screenshots or logs.
