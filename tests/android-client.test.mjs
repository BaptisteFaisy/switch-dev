import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("la cible Android reste actuelle, installable et adaptable", async () => {
  const [gradle, manifest, html, activity, strings] = await Promise.all([
    read("android/app/build.gradle"),
    read("android/app/src/main/AndroidManifest.xml"),
    read("index.html"),
    read("android/app/src/main/java/com/codexswitch/terminal/MainActivity.java"),
    read("android/app/src/main/res/values/strings.xml"),
  ]);

  assert.match(gradle, /compileSdk 36/);
  assert.match(gradle, /minSdk 24/);
  assert.match(gradle, /targetSdk 36/);
  assert.match(manifest, /android:windowSoftInputMode="adjustResize"/);
  assert.match(manifest, /android:hardwareAccelerated="true"/);
  assert.match(manifest, /android:enableOnBackInvokedCallback="true"/);
  assert.match(manifest, /android:configChanges="[^"]*orientation[^"]*screenSize/);
  assert.match(html, /interactive-widget=resizes-content/);
  assert.match(html, /viewport-fit=cover/);
  assert.match(strings, /<string name="server_url">https:\/\/azure-duello\.tail3a8bdf\.ts\.net<\/string>/);
  assert.match(strings, /<string name="server_start_url">https:\/\/azure-duello\.tail3a8bdf\.ts\.net\/\?cst-chunk-build=azure-20260826<\/string>/);
  assert.match(activity, /LEGACY_VPS_BASE_URL = "https:\/\/azure-duello\.tail3a8bdf\.ts\.net"/);
  assert.match(activity, /LEGACY_PC_BASE_URL = "https:\/\/pc-fixe-cst\.tail3a8bdf\.ts\.net"/);
  assert.match(activity, /KEY_AZURE_ROUTE_MIGRATED = "azureRouteMigrated20260826"/);
  assert.match(activity, /migrateLegacyServerRoute\(\)/);
  assert.match(activity, /migrateAzureServerRoute\(\)/);
  assert.match(activity, /LEGACY_PC_BASE_URL\.equals\(saved\)/);
  assert.match(activity, /webView\.loadUrl\(currentStartupUrl\(\)\)/);
  assert.match(activity, /buildUpon\(\)\.clearQuery\(\)\.fragment\(null\)/);
});

test("la coque Android durcit la WebView et protege la configuration", async () => {
  const [activity, tokenStore, manifest, extractionRules, legacyRules] = await Promise.all([
    read("android/app/src/main/java/com/codexswitch/terminal/MainActivity.java"),
    read("android/app/src/main/java/com/codexswitch/terminal/SecureTokenStore.java"),
    read("android/app/src/main/AndroidManifest.xml"),
    read("android/app/src/main/res/xml/data_extraction_rules.xml"),
    read("android/app/src/main/res/xml/full_backup_rules.xml"),
  ]);

  assert.match(activity, /setMixedContentMode\(WebSettings\.MIXED_CONTENT_NEVER_ALLOW\)/);
  assert.match(activity, /setSafeBrowsingEnabled\(true\)/);
  assert.match(activity, /setAcceptThirdPartyCookies\(view, false\)/);
  assert.match(activity, /setLayerType\(View\.LAYER_TYPE_HARDWARE, null\)/);
  assert.match(activity, /setRendererPriorityPolicy\(WebView\.RENDERER_PRIORITY_BOUND, true\)/);
  assert.match(activity, /setOverScrollMode\(View\.OVER_SCROLL_NEVER\)/);
  assert.match(activity, /"https"\.equalsIgnoreCase\(parsed\.getScheme\(\)\)/);
  assert.match(activity, /handler\.cancel\(\)/);
  assert.match(tokenStore, /AndroidKeyStore/);
  assert.match(tokenStore, /AES\/GCM\/NoPadding/);
  assert.match(tokenStore, /KeyProperties\.PURPOSE_ENCRYPT \| KeyProperties\.PURPOSE_DECRYPT/);
  assert.match(manifest, /android:usesCleartextTraffic="false"/);
  assert.match(manifest, /android:allowBackup="false"/);
  assert.match(manifest, /android:dataExtractionRules="@xml\/data_extraction_rules"/);
  assert.match(extractionRules, /<device-transfer>/);
  assert.match(extractionRules, /<exclude domain="sharedpref" path="\."/);
  assert.match(legacyRules, /<full-backup-content>/);
});

test("la coque Android couvre recuperation, fichiers et multimedia", async () => {
  const [activity, manifest] = await Promise.all([
    read("android/app/src/main/java/com/codexswitch/terminal/MainActivity.java"),
    read("android/app/src/main/AndroidManifest.xml"),
  ]);

  for (const method of ["getBaseUrl", "getToken", "setConfig", "openSettings", "openPaymentSettings", "openGooglePaySettings", "openExternalHttpsUrl", "consumePaymentHandoff"]) {
    assert.match(activity, new RegExp(`public (?:String|void|boolean) ${method}\\(`));
  }
  assert.match(activity, /configureErrorOverlay\(\)/);
  assert.match(activity, /showSettingsDialog\(\)/);
  assert.match(activity, /onShowFileChooser\(/);
  assert.match(activity, /PermissionRequest\.RESOURCE_AUDIO_CAPTURE/);
  assert.match(activity, /PermissionRequest\.RESOURCE_VIDEO_CAPTURE/);
  assert.match(activity, /setDownloadListener\(this::handleDownload\)/);
  assert.match(activity, /saveBase64File\(/);
  assert.match(activity, /WindowInsets\.Type\.ime\(\)/);
  assert.match(activity, /registerOnBackInvokedCallback\(/);
  assert.match(manifest, /android\.permission\.RECORD_AUDIO/);
  assert.match(manifest, /android\.permission\.CAMERA/);
  assert.match(manifest, /android\.permission\.MODIFY_AUDIO_SETTINGS/);
  assert.match(manifest, /android\.hardware\.camera" android:required="false"/);
  assert.match(manifest, /android\.hardware\.camera\.any" android:required="false"/);
});

test("les parametres web peuvent ouvrir la configuration native", async () => {
  const [platform, main] = await Promise.all([
    read("src/platform.ts"),
    read("src/main.ts"),
  ]);

  assert.match(platform, /export const hasMobileSettings/);
  assert.match(platform, /export const openMobileSettings/);
  assert.match(main, /id="settingsMobileConnection"/);
  assert.match(main, /openMobileSettings\(\)/);
});

test("les paiements autonomes produisent un handoff FCM prive et verifie", async () => {
  const [
    rootGradle,
    appGradle,
    gradleProperties,
    manifest,
    activity,
    registration,
    googlePaySettings,
    messagingService,
    application,
    platform,
    main,
    gitignore,
  ] = await Promise.all([
    read("android/build.gradle"),
    read("android/app/build.gradle"),
    read("android/gradle.properties"),
    read("android/app/src/main/AndroidManifest.xml"),
    read("android/app/src/main/java/com/codexswitch/terminal/MainActivity.java"),
    read("android/app/src/main/java/com/codexswitch/terminal/PaymentPushRegistration.java"),
    read("android/app/src/main/java/com/codexswitch/terminal/GooglePaySettings.java"),
    read("android/app/src/main/java/com/codexswitch/terminal/PaymentMessagingService.java"),
    read("android/app/src/main/java/com/codexswitch/terminal/CstApplication.java"),
    read("src/platform.ts"),
    read("src/main.ts"),
    read(".gitignore"),
  ]);

  assert.match(rootGradle, /com\.google\.gms\.google-services' version '4\.5\.0'/);
  assert.match(appGradle, /googleServicesConfigured/);
  assert.match(appGradle, /firebase-bom:34\.15\.0/);
  assert.match(appGradle, /firebase-messaging/);
  assert.match(appGradle, /play-services-wallet:20\.0\.0/);
  assert.match(gradleProperties, /android\.useAndroidX=true/);
  assert.match(manifest, /android\.permission\.POST_NOTIFICATIONS/);
  assert.match(manifest, /firebase_messaging_installation_id_enabled/);
  assert.match(manifest, /com\.google\.firebase\.MESSAGING_EVENT/);
  assert.match(messagingService, /void onRegistered\(String firebaseInstallationId\)/);
  assert.match(messagingService, /void onMessageReceived\(RemoteMessage message\)/);
  assert.match(registration, /FirebaseMessaging\.getInstance\(\)\.register\(\)/);
  assert.match(registration, /new FirebaseOptions\.Builder\(\)/);
  assert.match(registration, /\/api\/notifications\/mobile-push" \+ endpointSuffix/);
  assert.match(registration, /"\/config", "GET"/);
  assert.match(registration, /"\/test", "POST"/);
  assert.match(registration, /"configuration_test"/);
  assert.match(registration, /\/api\/notifications\/mobile-push\/devices/);
  assert.match(registration, /"Authorization", "Bearer " \+ adminToken/);
  assert.match(registration, /Notification\.VISIBILITY_PRIVATE/);
  assert.match(registration, /WindowManager\.LayoutParams\.FLAG_SECURE/);
  assert.match(registration, /"serviceAccountJson"/);
  assert.match(registration, /EXTRA_AGENT_ID/);
  assert.match(registration, /EXTRA_PAYMENT_ID/);
  assert.doesNotMatch(registration, /checkoutUrl|checkout_url/);
  assert.match(application, /initializeFirebaseFromStoredConfiguration\(this\)/);
  assert.match(manifest, /android:name="\.CstApplication"/);
  assert.match(activity, /public String consumePaymentHandoff\(\)/);
  assert.match(activity, /public void openPaymentSettings\(\)/);
  assert.match(activity, /public void openGooglePaySettings\(\)/);
  assert.match(activity, /public boolean openExternalHttpsUrl\(String rawUrl\)/);
  assert.match(activity, /!"https"\.equalsIgnoreCase\(uri\.getScheme\(\)\)/);
  assert.match(googlePaySettings, /WalletConstants\.ENVIRONMENT_PRODUCTION/);
  assert.match(googlePaySettings, /paymentsClient\.isReadyToPay\(request\)/);
  assert.match(googlePaySettings, /"existingPaymentMethodRequired", true/);
  assert.match(googlePaySettings, /com\.google\.android\.apps\.walletnfcrel/);
  assert.match(googlePaySettings, /https:\/\/wallet\.google\.com\//);
  assert.doesNotMatch(googlePaySettings, /SharedPreferences|cardNumber|cryptogram|privateKey/);
  assert.match(activity, /__cstPaymentHandoffReady/);
  assert.match(platform, /installMobilePaymentHandoffListener/);
  assert.match(platform, /openMobilePaymentSettings/);
  assert.match(platform, /openMobileGooglePaySettings/);
  assert.match(platform, /mobileBridge\(\)\?\.openExternalHttpsUrl/);
  assert.match(platform, /paymentHandoffIdPattern/);
  assert.match(main, /pendingPayment\?\.id === handoff\.paymentId/);
  assert.match(main, /pendingPayment\.status === "pending"/);
  assert.match(main, /id="settingsMobilePayments"/);
  assert.match(main, /id="settingsMobileGooglePay"/);
  assert.match(gitignore, /android\/app\/google-services\.json/);
});

test("la coque Android permet de coder dans les terminaux sur mobile", async () => {
  const [activity, helper] = await Promise.all([
    read("android/app/src/main/java/com/codexswitch/terminal/MainActivity.java"),
    read("android/app/src/main/assets/cst-mobile-terminal-helper.js"),
  ]);

  // Injection du script depuis les assets, une fois par page.
  assert.match(activity, /MOBILE_TERMINAL_HELPER_ASSET = "cst-mobile-terminal-helper\.js"/);
  assert.match(activity, /injectMobileTerminalHelper\(\)/);
  assert.match(activity, /webView\.evaluateJavascript\(mobileTerminalHelperScript, null\)/);
  assert.match(activity, /onPageFinished/);
  assert.match(activity, /getAssets\(\)\.open\(name\)/);

  // Ouverture du clavier virtuel depuis la page (pont natif).
  assert.match(activity, /public void showKeyboard\(\)/);
  assert.match(activity, /public void hideKeyboard\(\)/);
  assert.match(activity, /InputMethodManager\.SHOW_IMPLICIT/);
  assert.match(activity, /Api30Insets\.showIme\(webView\)/);
  assert.match(activity, /WindowInsets\.Type\.ime\(\)/);

  // La barre de touches existe, est idempotente et ne s'active que sur tactile.
  assert.match(helper, /__cstMobileTerminalHelper/);
  assert.match(helper, /cst-term-toolbar/);
  assert.match(helper, /ontouchstart/);
  assert.match(helper, /data-terminal-host/);
  assert.match(helper, /aria-modal="true"/);

  // La barre tient sur une seule ligne. Ctrl+V remplace Entree et passe par
  // un raccourci Android natif pour produire le vrai evenement paste de xterm.
  for (const key of ["Escape", "Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"]) {
    assert.match(helper, new RegExp(key));
  }
  assert.match(helper, /makeButton\("Ctrl\+V", "Ctrl\+V — coller"/);
  assert.match(helper, /nativeBridge\.pasteFromClipboard\(\)/);
  assert.match(activity, /public void pasteFromClipboard\(\)/);
  assert.match(activity, /KeyEvent\.KEYCODE_V/);
  assert.match(activity, /KeyEvent\.META_CTRL_ON/);
  assert.doesNotMatch(helper, /makeButton\("⏎"|"Entrée"/);
  assert.match(helper, /makeButton\("Ctrl\+C"/);
  for (const removed of ["Ctrl+D", "Ctrl+L", "Ctrl+Z", "Ctrl+U", "Ctrl+A", "Ctrl+W"]) {
    assert.doesNotMatch(helper, new RegExp(removed.replace("+", "\\+")));
  }
  assert.doesNotMatch(helper, /makeButton\("⌫"|var row2|keyboardButton|data-cst-chip/);
  assert.match(helper, /flex-wrap:nowrap/);

  // xterm reçoit les séquences directement via son événement input : les
  // flèches ne peuvent donc pas déclencher les raccourcis globaux de l'app.
  assert.match(helper, /xterm-helper-textarea/);
  assert.match(helper, /new InputEvent\("input"/);
  assert.match(helper, /inputType: "insertText"/);
  assert.match(helper, /textarea\.dispatchEvent\(event\);\s*return true;/);
  assert.match(helper, /event\.stopImmediatePropagation\(\)/);

  // Un ancien WebView reste utilisable grâce au KeyboardEvent de repli.
  assert.match(helper, /new KeyboardEvent\(["']keydown["']/);
  assert.match(helper, /dispatchEvent\(new KeyboardEvent/);

  // Les modificateurs one-shot Ctrl/Alt restent visuels et s'auto-desarment.
  assert.match(helper, /armModifier\(/);
  assert.match(helper, /disarmModifiers\(\)/);
  assert.match(helper, /MODIFIER_DISARM_TIMEOUT_MS/);
});
