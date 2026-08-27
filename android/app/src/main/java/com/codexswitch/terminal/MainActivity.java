package com.codexswitch.terminal;

import android.Manifest;
import android.annotation.SuppressLint;
import android.app.Activity;
import android.app.AlertDialog;
import android.app.DownloadManager;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.net.http.SslError;
import android.os.Bundle;
import android.os.Environment;
import android.os.SystemClock;
import android.text.InputType;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.PermissionRequest;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.SslErrorHandler;
import android.webkit.URLUtil;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.UUID;

/**
 * Client Android de Codex Switch Terminal.
 *
 * L'application charge le frontend de cst-server dans une WebView durcie et
 * expose le contrat window.CstAndroid attendu par src/platform.ts. Elle ajoute
 * les integrations qu'une WebView nue ne fournit pas : recuperation reseau,
 * configuration native, fichiers, telechargements et permissions multimedia.
 */
public class MainActivity extends Activity {

    private static final String PREFS = "cst";
    private static final String KEY_BASE = "baseUrl";
    private static final String KEY_PC_ROUTE_MIGRATED = "pcRouteMigrated20260817";
    private static final String KEY_AZURE_ROUTE_MIGRATED = "azureRouteMigrated20260826";
    private static final String LEGACY_VPS_BASE_URL = "https://azure-duello.tail3a8bdf.ts.net";
    private static final String LEGACY_PC_BASE_URL = "https://pc-fixe-cst.tail3a8bdf.ts.net";
    private static final String LEGACY_KEY_TOKEN = "token";
    private static final int REQUEST_FILE_CHOOSER = 1001;
    private static final int REQUEST_WEB_PERMISSIONS = 1002;
    private static final int REQUEST_SAVE_FILE = 1003;
    private static final int MAX_BRIDGE_FILE_BASE64_LENGTH = 32 * 1024 * 1024;
    private static final String MOBILE_TERMINAL_HELPER_ASSET = "cst-mobile-terminal-helper.js";

    private FrameLayout rootView;
    private WebView webView;
    private ProgressBar progressBar;
    private View errorOverlay;
    private TextView errorMessage;
    private SharedPreferences preferences;
    private SecureTokenStore tokenStore;
    private ValueCallback<Uri[]> fileChooserCallback;
    private PermissionRequest pendingPermissionRequest;
    private String[] pendingPermissionResources;
    private byte[] pendingSaveData;
    private String pendingSaveName;
    private String pendingSaveMime;
    private boolean mainFrameFailed;
    private String pendingPaymentAgentId;
    private String pendingPaymentId;
    private String pendingAutonomousAgentId;
    private String mobileTerminalHelperScript;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        preferences = getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        tokenStore = new SecureTokenStore(preferences);
        migrateLegacyToken();
        migrateLegacyServerRoute();
        migrateAzureServerRoute();
        capturePaymentHandoff(getIntent());
        captureAutonomousAgentHandoff(getIntent());

        rootView = new FrameLayout(this);
        rootView.setBackgroundColor(Color.BLACK);
        setContentView(rootView);
        configureWindowInsets();
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            Api33Back.register(this);
        }

        webView = createWebView();
        rootView.addView(webView, matchParentLayout());
        configureProgressBar();
        configureErrorOverlay();

        if (savedInstanceState == null || webView.restoreState(savedInstanceState) == null) {
            loadConfiguredServer();
        }
        PaymentPushRegistration.initialize(this);
    }

    private FrameLayout.LayoutParams matchParentLayout() {
        return new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT);
    }

    @SuppressLint("SetJavaScriptEnabled")
    private WebView createWebView() {
        WebView view = new WebView(this);
        view.setBackgroundColor(Color.BLACK);
        view.setFocusable(true);
        view.setFocusableInTouchMode(true);
        view.setLayerType(View.LAYER_TYPE_HARDWARE, null);
        view.setOverScrollMode(View.OVER_SCROLL_NEVER);

        WebSettings settings = view.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setMediaPlaybackRequiresUserGesture(false);
        settings.setSupportZoom(false);
        settings.setBuiltInZoomControls(false);
        settings.setDisplayZoomControls(false);
        settings.setUseWideViewPort(true);
        settings.setLoadWithOverviewMode(false);
        settings.setCacheMode(WebSettings.LOAD_DEFAULT);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(true);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setTextZoom(100);
        settings.setUserAgentString(
                settings.getUserAgentString() + " CodexTerminalAndroid/1.0");
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
            settings.setSafeBrowsingEnabled(true);
            view.setRendererPriorityPolicy(WebView.RENDERER_PRIORITY_BOUND, true);
        }

        CookieManager cookies = CookieManager.getInstance();
        cookies.setAcceptCookie(true);
        cookies.setAcceptThirdPartyCookies(view, false);

        WebView.setWebContentsDebuggingEnabled(
                (getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0);
        view.addJavascriptInterface(new CstBridge(), "CstAndroid");
        view.setWebViewClient(android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O
                ? new OreoWebViewClient()
                : new CstWebViewClient());
        view.setWebChromeClient(new CstWebChromeClient());
        view.setDownloadListener(this::handleDownload);
        return view;
    }

    private void configureProgressBar() {
        progressBar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progressBar.setMax(100);
        progressBar.setProgressTintList(ColorStateList.valueOf(getColor(R.color.accent)));
        progressBar.setProgressBackgroundTintList(ColorStateList.valueOf(Color.TRANSPARENT));
        progressBar.setVisibility(View.GONE);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                dp(3),
                Gravity.TOP);
        rootView.addView(progressBar, params);
    }

    private void configureErrorOverlay() {
        LinearLayout overlay = new LinearLayout(this);
        overlay.setOrientation(LinearLayout.VERTICAL);
        overlay.setGravity(Gravity.CENTER);
        overlay.setPadding(dp(28), dp(32), dp(28), dp(32));
        overlay.setBackgroundColor(Color.rgb(8, 8, 8));
        overlay.setVisibility(View.GONE);

        TextView title = new TextView(this);
        title.setText(R.string.connection_error_title);
        title.setTextColor(Color.WHITE);
        title.setTextSize(23);
        title.setGravity(Gravity.CENTER);
        title.setTypeface(title.getTypeface(), android.graphics.Typeface.BOLD);
        overlay.addView(title, wrapContentLayout());

        errorMessage = new TextView(this);
        errorMessage.setTextColor(Color.rgb(190, 190, 190));
        errorMessage.setTextSize(14);
        errorMessage.setGravity(Gravity.CENTER);
        errorMessage.setLineSpacing(0, 1.12f);
        LinearLayout.LayoutParams messageParams = wrapContentLayout();
        messageParams.topMargin = dp(12);
        messageParams.bottomMargin = dp(24);
        overlay.addView(errorMessage, messageParams);

        Button retry = nativeButton(getString(R.string.retry), true);
        retry.setOnClickListener(ignored -> loadConfiguredServer());
        overlay.addView(retry, buttonLayout());

        Button configure = nativeButton(getString(R.string.configure), false);
        configure.setOnClickListener(ignored -> showSettingsDialog());
        LinearLayout.LayoutParams configureParams = buttonLayout();
        configureParams.topMargin = dp(10);
        overlay.addView(configure, configureParams);

        errorOverlay = overlay;
        rootView.addView(errorOverlay, matchParentLayout());
    }

    private LinearLayout.LayoutParams wrapContentLayout() {
        return new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
    }

    private LinearLayout.LayoutParams buttonLayout() {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                dp(50));
        params.setMargins(dp(12), 0, dp(12), 0);
        return params;
    }

    private Button nativeButton(String label, boolean primary) {
        Button button = new Button(this);
        button.setAllCaps(false);
        button.setText(label);
        button.setTextSize(16);
        button.setTextColor(primary ? Color.BLACK : Color.WHITE);
        GradientDrawable background = new GradientDrawable();
        background.setColor(primary ? getColor(R.color.accent) : Color.rgb(32, 32, 32));
        background.setCornerRadius(dp(10));
        background.setStroke(dp(1), primary ? getColor(R.color.accent) : Color.rgb(65, 65, 65));
        button.setBackground(background);
        return button;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private void configureWindowInsets() {
        rootView.setOnApplyWindowInsetsListener((view, insets) -> {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                Api30Insets.apply(view, insets);
            } else {
                view.setPadding(
                        insets.getSystemWindowInsetLeft(),
                        insets.getSystemWindowInsetTop(),
                        insets.getSystemWindowInsetRight(),
                        insets.getSystemWindowInsetBottom());
            }
            return insets;
        });
        rootView.requestApplyInsets();
    }

    private void migrateLegacyToken() {
        String legacy = preferences.getString(LEGACY_KEY_TOKEN, "");
        if (legacy == null || legacy.trim().isEmpty()) {
            return;
        }
        if (tokenStore.read().isEmpty()) {
            tokenStore.write(legacy);
        }
        preferences.edit().remove(LEGACY_KEY_TOKEN).apply();
    }

    /**
     * Bascule une seule fois les installations existantes de l'ancien VPS vers
     * la cible embarquee. Les URL personnalisees restent intactes et
     * l'utilisateur peut toujours choisir une autre cible apres cette migration.
     */
    private void migrateLegacyServerRoute() {
        if (preferences.getBoolean(KEY_PC_ROUTE_MIGRATED, false)) {
            return;
        }

        SharedPreferences.Editor editor = preferences.edit()
                .putBoolean(KEY_PC_ROUTE_MIGRATED, true);
        String saved = normalizeServerUrl(preferences.getString(KEY_BASE, ""));
        String bundled = normalizeServerUrl(getString(R.string.server_url));
        if (LEGACY_VPS_BASE_URL.equals(saved) && bundled != null) {
            editor.putString(KEY_BASE, bundled);
        }
        editor.apply();
    }

    /**
     * Bascule une seule fois l'ancien routage par defaut du PC de developpement
     * vers le VPS Azure. Une cible personnalisee reste intacte, y compris si
     * l'utilisateur la choisit apres cette migration.
     */
    private void migrateAzureServerRoute() {
        if (preferences.getBoolean(KEY_AZURE_ROUTE_MIGRATED, false)) {
            return;
        }

        SharedPreferences.Editor editor = preferences.edit()
                .putBoolean(KEY_AZURE_ROUTE_MIGRATED, true);
        String saved = normalizeServerUrl(preferences.getString(KEY_BASE, ""));
        String bundled = normalizeServerUrl(getString(R.string.server_url));
        if (LEGACY_PC_BASE_URL.equals(saved) && bundled != null) {
            editor.putString(KEY_BASE, bundled);
        }
        editor.apply();
    }

    private String currentBaseUrl() {
        String saved = preferences.getString(KEY_BASE, "");
        String normalizedSaved = normalizeServerUrl(saved);
        if (normalizedSaved != null) {
            return normalizedSaved;
        }
        if (saved != null && !saved.trim().isEmpty()) {
            preferences.edit().remove(KEY_BASE).apply();
        }
        String bundled = normalizeServerUrl(getString(R.string.server_url));
        if (bundled == null) {
            throw new IllegalStateException("L'URL serveur embarquee doit utiliser HTTPS.");
        }
        return bundled;
    }

    /**
     * Le pont JavaScript conserve une URL de base sans query string pour les
     * appels API. Seule la navigation initiale ajoute l'identifiant du build
     * frontend demande, et uniquement pour la cible PC embarquee.
     */
    private String currentStartupUrl() {
        String base = currentBaseUrl();
        String bundled = normalizeServerUrl(getString(R.string.server_url));
        String startup = getString(R.string.server_start_url).trim();
        if (base.equals(bundled) && base.equals(normalizeServerUrl(startup))) {
            return startup;
        }
        return base;
    }

    /** N'accepte que HTTPS, sans identifiants, query string ni fragment. */
    private String normalizeServerUrl(String rawValue) {
        if (rawValue == null) {
            return null;
        }
        String value = rawValue.trim();
        if (value.isEmpty()) {
            return null;
        }
        try {
            Uri parsed = Uri.parse(value);
            if (!"https".equalsIgnoreCase(parsed.getScheme())
                    || parsed.getHost() == null
                    || parsed.getHost().trim().isEmpty()
                    || parsed.getUserInfo() != null) {
                return null;
            }
            Uri.Builder builder = parsed.buildUpon().clearQuery().fragment(null);
            if ("/".equals(parsed.getPath())) {
                builder.path("");
            }
            String normalized = builder.build().toString();
            while (normalized.endsWith("/")) {
                normalized = normalized.substring(0, normalized.length() - 1);
            }
            return normalized;
        } catch (Exception ignored) {
            return null;
        }
    }

    private void loadConfiguredServer() {
        mainFrameFailed = false;
        hideConnectionError();
        progressBar.setProgress(5);
        progressBar.setVisibility(View.VISIBLE);
        webView.loadUrl(currentStartupUrl());
    }

    private void showConnectionError(String details) {
        mainFrameFailed = true;
        progressBar.setVisibility(View.GONE);
        errorMessage.setText(getString(
                R.string.connection_error_details,
                details,
                getString(R.string.connection_error_hint)));
        errorOverlay.setVisibility(View.VISIBLE);
        errorOverlay.bringToFront();
    }

    private void hideConnectionError() {
        if (errorOverlay != null) {
            errorOverlay.setVisibility(View.GONE);
        }
    }

    private void showSettingsDialog() {
        if (isFinishing()) {
            return;
        }

        LinearLayout fields = new LinearLayout(this);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(dp(4), dp(8), dp(4), 0);

        EditText baseUrl = new EditText(this);
        baseUrl.setHint("https://serveur.example.com");
        baseUrl.setSingleLine(true);
        baseUrl.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        baseUrl.setText(currentBaseUrl());
        baseUrl.setSelectAllOnFocus(true);
        fields.addView(baseUrl, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        EditText token = new EditText(this);
        token.setHint(R.string.admin_token);
        token.setSingleLine(true);
        token.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        token.setText(tokenStore.read());
        LinearLayout.LayoutParams tokenParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        tokenParams.topMargin = dp(8);
        fields.addView(token, tokenParams);

        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle(R.string.connection_settings_title)
                .setMessage(R.string.connection_settings_message)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.payment_config_open, null)
                .setPositiveButton(R.string.save, null)
                .create();
        dialog.setOnShowListener(ignored -> {
            dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(button -> {
                dialog.dismiss();
                PaymentPushRegistration.showConfigurationDialog(this);
            });
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(button -> {
                    String previousBaseUrl = currentBaseUrl();
                    String previousAdminToken = tokenStore.read();
                    String normalized = normalizeServerUrl(baseUrl.getText().toString());
                    if (normalized == null) {
                        baseUrl.setError(getString(R.string.https_required));
                        baseUrl.requestFocus();
                        return;
                    }
                    try {
                        preferences.edit().putString(KEY_BASE, normalized).apply();
                        tokenStore.write(token.getText().toString());
                    } catch (RuntimeException error) {
                        token.setError(error.getMessage());
                        return;
                    }
                    dialog.dismiss();
                    webView.clearHistory();
                    loadConfiguredServer();
                    PaymentPushRegistration.refreshAfterConfigurationChange(
                            this,
                            preferences,
                            tokenStore,
                            previousBaseUrl,
                            previousAdminToken);
                });
        });
        dialog.show();
    }

    private boolean isInternalUri(Uri target) {
        if (target == null || target.getHost() == null) {
            return false;
        }
        String targetHost = target.getHost();
        Uri configured = Uri.parse(currentBaseUrl());
        if (targetHost.equalsIgnoreCase(configured.getHost())) {
            return true;
        }
        String current = webView == null ? null : webView.getUrl();
        Uri currentUri = current == null ? null : Uri.parse(current);
        return currentUri != null
                && currentUri.getHost() != null
                && targetHost.equalsIgnoreCase(currentUri.getHost());
    }

    private boolean openExternalUri(Uri uri) {
        try {
            Intent intent;
            if ("intent".equalsIgnoreCase(uri.getScheme())) {
                intent = Intent.parseUri(uri.toString(), Intent.URI_INTENT_SCHEME);
            } else {
                intent = new Intent(Intent.ACTION_VIEW, uri);
            }
            startActivity(intent);
            return true;
        } catch (Exception ignored) {
            Toast.makeText(this, R.string.no_app_for_link, Toast.LENGTH_SHORT).show();
            return false;
        }
    }

    private String validUuid(String value) {
        if (value == null) {
            return "";
        }
        try {
            return UUID.fromString(value.trim()).toString();
        } catch (IllegalArgumentException ignored) {
            return "";
        }
    }

    private void capturePaymentHandoff(Intent intent) {
        if (intent == null
                || !PaymentPushRegistration.ACTION_OPEN_PAYMENT.equals(intent.getAction())) {
            return;
        }
        String agentId = validUuid(intent.getStringExtra(PaymentPushRegistration.EXTRA_AGENT_ID));
        String paymentId = validUuid(intent.getStringExtra(PaymentPushRegistration.EXTRA_PAYMENT_ID));
        if (agentId.isEmpty() || paymentId.isEmpty()) {
            return;
        }
        synchronized (this) {
            pendingPaymentAgentId = agentId;
            pendingPaymentId = paymentId;
        }
    }

    private String consumePendingPaymentHandoff() {
        synchronized (this) {
            if (pendingPaymentAgentId == null || pendingPaymentId == null) {
                return "";
            }
            try {
                return new JSONObject()
                        .put("agentId", pendingPaymentAgentId)
                        .put("paymentId", pendingPaymentId)
                        .toString();
            } catch (Exception ignored) {
                return "";
            } finally {
                pendingPaymentAgentId = null;
                pendingPaymentId = null;
            }
        }
    }

    private void dispatchPendingPaymentHandoff() {
        final String agentId;
        final String paymentId;
        synchronized (this) {
            agentId = pendingPaymentAgentId;
            paymentId = pendingPaymentId;
        }
        if (webView == null || agentId == null || paymentId == null) {
            return;
        }
        final String detail;
        try {
            detail = new JSONObject()
                    .put("agentId", agentId)
                    .put("paymentId", paymentId)
                    .toString();
        } catch (Exception ignored) {
            return;
        }
        String script = "(function(){if(window.__cstPaymentHandoffReady!==true)return false;"
                + "window.dispatchEvent(new CustomEvent('cst:payment-handoff',{detail:"
                + detail
                + "}));return true;})()";
        webView.evaluateJavascript(script, result -> {
            if (!"true".equals(result)) {
                return;
            }
            synchronized (MainActivity.this) {
                if (agentId.equals(pendingPaymentAgentId)
                        && paymentId.equals(pendingPaymentId)) {
                    pendingPaymentAgentId = null;
                    pendingPaymentId = null;
                }
            }
        });
    }

    private void captureAutonomousAgentHandoff(Intent intent) {
        if (intent == null
                || !PaymentPushRegistration.ACTION_OPEN_AUTONOMOUS_AGENT.equals(intent.getAction())) {
            return;
        }
        String agentId = validUuid(
                intent.getStringExtra(PaymentPushRegistration.EXTRA_AUTONOMOUS_AGENT_ID));
        if (agentId.isEmpty()) {
            return;
        }
        synchronized (this) {
            pendingAutonomousAgentId = agentId;
        }
    }

    private String consumePendingAutonomousAgentHandoff() {
        synchronized (this) {
            if (pendingAutonomousAgentId == null) {
                return "";
            }
            try {
                return new JSONObject()
                        .put("agentId", pendingAutonomousAgentId)
                        .toString();
            } catch (Exception ignored) {
                return "";
            } finally {
                pendingAutonomousAgentId = null;
            }
        }
    }

    private void dispatchPendingAutonomousAgentHandoff() {
        final String agentId;
        synchronized (this) {
            agentId = pendingAutonomousAgentId;
        }
        if (webView == null || agentId == null) {
            return;
        }
        final String detail;
        try {
            detail = new JSONObject().put("agentId", agentId).toString();
        } catch (Exception ignored) {
            return;
        }
        String script = "(function(){if(window.__cstAutonomousAgentHandoffReady!==true)return false;"
                + "window.dispatchEvent(new CustomEvent('cst:autonomous-agent-handoff',{detail:"
                + detail
                + "}));return true;})()";
        webView.evaluateJavascript(script, result -> {
            if (!"true".equals(result)) {
                return;
            }
            synchronized (MainActivity.this) {
                if (agentId.equals(pendingAutonomousAgentId)) {
                    pendingAutonomousAgentId = null;
                }
            }
        });
    }

    private void handleDownload(
            String url,
            String userAgent,
            String contentDisposition,
            String mimeType,
            long contentLength) {
        String filename = safeFilename(URLUtil.guessFileName(url, contentDisposition, mimeType));
        if (url != null && url.startsWith("blob:")) {
            captureBlobDownload(url, filename, mimeType);
            return;
        }
        if (url == null || !"https".equalsIgnoreCase(Uri.parse(url).getScheme())) {
            Toast.makeText(this, R.string.download_blocked, Toast.LENGTH_LONG).show();
            return;
        }

        try {
            DownloadManager.Request request = new DownloadManager.Request(Uri.parse(url));
            request.setTitle(filename);
            request.setDescription(getString(R.string.download_description));
            request.setMimeType(mimeType == null || mimeType.isEmpty()
                    ? "application/octet-stream"
                    : mimeType);
            request.setAllowedOverMetered(true);
            request.setAllowedOverRoaming(false);
            request.setNotificationVisibility(
                    DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED);
            String cookies = CookieManager.getInstance().getCookie(url);
            if (cookies != null && !cookies.isEmpty()) {
                request.addRequestHeader("Cookie", cookies);
            }
            if (userAgent != null && !userAgent.isEmpty()) {
                request.addRequestHeader("User-Agent", userAgent);
            }
            request.setDestinationInExternalFilesDir(
                    this,
                    Environment.DIRECTORY_DOWNLOADS,
                    System.currentTimeMillis() + "-" + filename);
            DownloadManager manager = (DownloadManager) getSystemService(DOWNLOAD_SERVICE);
            manager.enqueue(request);
            Toast.makeText(this, R.string.download_started, Toast.LENGTH_SHORT).show();
        } catch (Exception error) {
            Toast.makeText(this, getString(R.string.download_failed, error.getMessage()), Toast.LENGTH_LONG).show();
        }
    }

    private void captureBlobDownload(String blobUrl, String filename, String mimeType) {
        String script = "(async()=>{try{const r=await fetch(" + JSONObject.quote(blobUrl)
                + ");const b=await r.blob();const q=new FileReader();q.onloadend=()=>{const d=String(q.result||'');"
                + "window.CstAndroid.saveBase64File(" + JSONObject.quote(filename) + ","
                + JSONObject.quote(mimeType == null ? "application/octet-stream" : mimeType)
                + ",d.slice(d.indexOf(',')+1));};q.readAsDataURL(b);}catch(e){window.CstAndroid.fileSaveFailed(String(e));}})();";
        webView.evaluateJavascript(script, null);
    }

    private String safeFilename(String raw) {
        String value = raw == null || raw.trim().isEmpty() ? "codex-terminal-download" : raw.trim();
        value = value.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}]", "_");
        if (value.length() > 120) {
            value = value.substring(value.length() - 120);
        }
        return value;
    }

    private void startDocumentSave(String filename, String mimeType, byte[] data) {
        if (pendingSaveData != null) {
            Toast.makeText(this, R.string.save_in_progress, Toast.LENGTH_SHORT).show();
            return;
        }
        pendingSaveName = safeFilename(filename);
        pendingSaveMime = mimeType == null || mimeType.trim().isEmpty()
                ? "application/octet-stream"
                : mimeType;
        pendingSaveData = data;

        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType(pendingSaveMime);
        intent.putExtra(Intent.EXTRA_TITLE, pendingSaveName);
        try {
            startActivityForResult(intent, REQUEST_SAVE_FILE);
        } catch (ActivityNotFoundException error) {
            clearPendingSave();
            Toast.makeText(this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
        }
    }

    private void writePendingDocument(Uri destination) {
        byte[] data = pendingSaveData;
        String name = pendingSaveName;
        clearPendingSave();
        if (data == null || destination == null) {
            return;
        }
        new Thread(() -> {
            try (OutputStream output = getContentResolver().openOutputStream(destination, "w")) {
                if (output == null) {
                    throw new IllegalStateException("Destination indisponible");
                }
                output.write(data);
                runOnUiThread(() -> Toast.makeText(
                        this,
                        getString(R.string.file_saved, name),
                        Toast.LENGTH_SHORT).show());
            } catch (Exception error) {
                runOnUiThread(() -> Toast.makeText(
                        this,
                        getString(R.string.download_failed, error.getMessage()),
                        Toast.LENGTH_LONG).show());
            }
        }, "cst-save-file").start();
    }

    private void clearPendingSave() {
        pendingSaveData = null;
        pendingSaveName = null;
        pendingSaveMime = null;
    }

    /**
     * Injecte l'assistant mobile des terminaux (barre de touches au-dessus du
     * clavier virtuel, focalisation du champ xterm, raccourcis). Le script est
     * idempotent : il ne s'installe qu'une seule fois par page, y compris apres
     * les navigations internes du SPA.
     */
    private void injectMobileTerminalHelper() {
        if (webView == null) {
            return;
        }
        if (mobileTerminalHelperScript == null) {
            mobileTerminalHelperScript = readAsset(MOBILE_TERMINAL_HELPER_ASSET);
        }
        if (mobileTerminalHelperScript == null || mobileTerminalHelperScript.isEmpty()) {
            return;
        }
        webView.evaluateJavascript(mobileTerminalHelperScript, null);
    }

    private String readAsset(String name) {
        try (InputStream input = getAssets().open(name)) {
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            byte[] chunk = new byte[16 * 1024];
            int read;
            while ((read = input.read(chunk)) != -1) {
                output.write(chunk, 0, read);
            }
            return output.toString("UTF-8");
        } catch (Exception error) {
            return null;
        }
    }

    /**
     * Ouvre ou ferme le clavier virtuel depuis la page. Sur API 30+, le chemin
     * WindowInsetsController est plus fiable que showSoftInput seul.
     */
    private void showSoftKeyboard(boolean show) {
        if (webView == null) {
            return;
        }
        InputMethodManager imm = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
        if (imm == null) {
            return;
        }
        if (show) {
            imm.showSoftInput(webView, InputMethodManager.SHOW_IMPLICIT);
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                try {
                    Api30Insets.showIme(webView);
                } catch (Exception ignored) {
                }
            }
        } else {
            imm.hideSoftInputFromWindow(webView.getWindowToken(), 0);
        }
    }

    /**
     * Colle dans l'element WebView actuellement focalise via le raccourci
     * Android natif. Contrairement a un KeyboardEvent JavaScript synthetique,
     * ce chemin autorise Chromium a emettre le vrai evenement paste attendu
     * par xterm, sans exposer le contenu du presse-papiers au pont JavaScript.
     */
    private void pasteIntoFocusedWebContent() {
        if (webView == null) {
            return;
        }
        webView.requestFocus();
        long now = SystemClock.uptimeMillis();
        int metaState = KeyEvent.META_CTRL_ON | KeyEvent.META_CTRL_LEFT_ON;
        webView.dispatchKeyEvent(new KeyEvent(
                now,
                now,
                KeyEvent.ACTION_DOWN,
                KeyEvent.KEYCODE_V,
                0,
                metaState));
        webView.dispatchKeyEvent(new KeyEvent(
                now,
                SystemClock.uptimeMillis(),
                KeyEvent.ACTION_UP,
                KeyEvent.KEYCODE_V,
                0,
                metaState));
    }

    private void handleWebPermissionRequest(PermissionRequest request) {
        runOnUiThread(() -> {
            if (!isInternalUri(request.getOrigin())) {
                request.deny();
                return;
            }

            List<String> grantableResources = new ArrayList<>();
            List<String> androidPermissions = new ArrayList<>();
            for (String resource : request.getResources()) {
                if (PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(resource)) {
                    grantableResources.add(resource);
                    if (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                            != PackageManager.PERMISSION_GRANTED) {
                        androidPermissions.add(Manifest.permission.RECORD_AUDIO);
                    }
                } else if (PermissionRequest.RESOURCE_VIDEO_CAPTURE.equals(resource)) {
                    grantableResources.add(resource);
                    if (checkSelfPermission(Manifest.permission.CAMERA)
                            != PackageManager.PERMISSION_GRANTED) {
                        androidPermissions.add(Manifest.permission.CAMERA);
                    }
                }
            }

            if (grantableResources.isEmpty()) {
                request.deny();
                return;
            }
            if (androidPermissions.isEmpty()) {
                request.grant(grantableResources.toArray(new String[0]));
                return;
            }

            if (pendingPermissionRequest != null) {
                pendingPermissionRequest.deny();
            }
            pendingPermissionRequest = request;
            pendingPermissionResources = grantableResources.toArray(new String[0]);
            requestPermissions(
                    androidPermissions.stream().distinct().toArray(String[]::new),
                    REQUEST_WEB_PERMISSIONS);
        });
    }

    @Override
    public void onRequestPermissionsResult(
            int requestCode,
            String[] permissions,
            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == PaymentPushRegistration.REQUEST_NOTIFICATION_PERMISSION) {
            boolean granted = grantResults.length > 0
                    && grantResults[0] == PackageManager.PERMISSION_GRANTED;
            PaymentPushRegistration.onNotificationPermissionResult(this, granted);
            return;
        }
        if (requestCode != REQUEST_WEB_PERMISSIONS || pendingPermissionRequest == null) {
            return;
        }

        List<String> granted = new ArrayList<>();
        for (String resource : pendingPermissionResources == null
                ? new String[0]
                : pendingPermissionResources) {
            if (PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(resource)
                    && checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                    == PackageManager.PERMISSION_GRANTED) {
                granted.add(resource);
            } else if (PermissionRequest.RESOURCE_VIDEO_CAPTURE.equals(resource)
                    && checkSelfPermission(Manifest.permission.CAMERA)
                    == PackageManager.PERMISSION_GRANTED) {
                granted.add(resource);
            }
        }
        if (granted.isEmpty()) {
            pendingPermissionRequest.deny();
        } else {
            pendingPermissionRequest.grant(granted.toArray(new String[0]));
        }
        pendingPermissionRequest = null;
        pendingPermissionResources = null;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQUEST_FILE_CHOOSER) {
            ValueCallback<Uri[]> callback = fileChooserCallback;
            fileChooserCallback = null;
            if (callback != null) {
                callback.onReceiveValue(
                        WebChromeClient.FileChooserParams.parseResult(resultCode, data));
            }
            return;
        }
        if (requestCode == REQUEST_SAVE_FILE) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                writePendingDocument(data.getData());
            } else {
                clearPendingSave();
            }
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        if (webView != null) {
            webView.saveState(outState);
        }
    }

    @Override
    protected void onPause() {
        if (webView != null) {
            webView.onPause();
        }
        CookieManager.getInstance().flush();
        super.onPause();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (webView != null) {
            webView.onResume();
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        capturePaymentHandoff(intent);
        captureAutonomousAgentHandoff(intent);
        dispatchPendingPaymentHandoff();
        dispatchPendingAutonomousAgentHandoff();
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (keyCode == KeyEvent.KEYCODE_BACK && webView != null && webView.canGoBack()) {
            webView.goBack();
            return true;
        }
        return super.onKeyDown(keyCode, event);
    }

    @Override
    public void onBackPressed() {
        handleBackNavigation();
    }

    private void handleBackNavigation() {
        if (webView != null && webView.canGoBack()) {
            webView.goBack();
        } else {
            finishAfterTransition();
        }
    }

    @Override
    protected void onDestroy() {
        if (fileChooserCallback != null) {
            fileChooserCallback.onReceiveValue(null);
            fileChooserCallback = null;
        }
        if (pendingPermissionRequest != null) {
            pendingPermissionRequest.deny();
            pendingPermissionRequest = null;
        }
        if (webView != null) {
            webView.removeJavascriptInterface("CstAndroid");
            webView.stopLoading();
            webView.destroy();
            webView = null;
        }
        super.onDestroy();
    }

    private void recreateWebViewAfterRendererCrash() {
        WebView crashed = webView;
        rootView.removeView(crashed);
        crashed.destroy();
        webView = createWebView();
        rootView.addView(webView, 0, matchParentLayout());
    }

    private class CstWebViewClient extends WebViewClient {
        @Override
        public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
            Uri url = request.getUrl();
            String scheme = url.getScheme() == null
                    ? ""
                    : url.getScheme().toLowerCase(Locale.ROOT);
            if (("https".equals(scheme) && isInternalUri(url)) || "about".equals(scheme)) {
                return false;
            }
            if ("https".equals(scheme)
                    || "mailto".equals(scheme)
                    || "tel".equals(scheme)
                    || "geo".equals(scheme)
                    || "market".equals(scheme)
                    || "intent".equals(scheme)) {
                openExternalUri(url);
            }
            return true;
        }

        @Override
        public void onPageStarted(WebView view, String url, android.graphics.Bitmap favicon) {
            mainFrameFailed = false;
            hideConnectionError();
            progressBar.setVisibility(View.VISIBLE);
        }

        @Override
        public void onPageFinished(WebView view, String url) {
            progressBar.setVisibility(View.GONE);
            if (!mainFrameFailed) {
                hideConnectionError();
            }
            dispatchPendingPaymentHandoff();
            dispatchPendingAutonomousAgentHandoff();
            injectMobileTerminalHelper();
        }

        @Override
        public void onReceivedError(
                WebView view,
                WebResourceRequest request,
                WebResourceError error) {
            if (request.isForMainFrame()) {
                showConnectionError(String.valueOf(error.getDescription()));
            }
        }

        @Override
        public void onReceivedHttpError(
                WebView view,
                WebResourceRequest request,
                WebResourceResponse errorResponse) {
            if (request.isForMainFrame() && errorResponse.getStatusCode() >= 400) {
                showConnectionError(getString(
                        R.string.http_error,
                        errorResponse.getStatusCode(),
                        errorResponse.getReasonPhrase()));
            }
        }

        @Override
        public void onReceivedSslError(WebView view, SslErrorHandler handler, SslError error) {
            handler.cancel();
            showConnectionError(getString(R.string.ssl_error));
        }

    }

    @android.annotation.TargetApi(android.os.Build.VERSION_CODES.O)
    private class OreoWebViewClient extends CstWebViewClient {
        @Override
        public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
            recreateWebViewAfterRendererCrash();
            showConnectionError(getString(R.string.webview_restarted));
            return true;
        }
    }

    private class CstWebChromeClient extends WebChromeClient {
        @Override
        public void onProgressChanged(WebView view, int newProgress) {
            progressBar.setProgress(newProgress);
            progressBar.setVisibility(newProgress >= 100 ? View.GONE : View.VISIBLE);
        }

        @Override
        public boolean onShowFileChooser(
                WebView view,
                ValueCallback<Uri[]> callback,
                FileChooserParams params) {
            if (fileChooserCallback != null) {
                fileChooserCallback.onReceiveValue(null);
            }
            fileChooserCallback = callback;
            Intent intent;
            try {
                intent = params.createIntent();
                intent.addCategory(Intent.CATEGORY_OPENABLE);
            } catch (Exception ignored) {
                intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType("*/*");
                intent.putExtra(
                        Intent.EXTRA_ALLOW_MULTIPLE,
                        params.getMode() == FileChooserParams.MODE_OPEN_MULTIPLE);
            }
            try {
                startActivityForResult(intent, REQUEST_FILE_CHOOSER);
                return true;
            } catch (ActivityNotFoundException error) {
                fileChooserCallback = null;
                callback.onReceiveValue(null);
                Toast.makeText(MainActivity.this, R.string.no_file_picker, Toast.LENGTH_LONG).show();
                return true;
            }
        }

        @Override
        public void onPermissionRequest(PermissionRequest request) {
            handleWebPermissionRequest(request);
        }

        @Override
        public void onPermissionRequestCanceled(PermissionRequest request) {
            if (pendingPermissionRequest == request) {
                pendingPermissionRequest = null;
                pendingPermissionResources = null;
            }
        }
    }

    /** Pont JavaScript <-> natif. Les noms font partie du contrat platform.ts. */
    private class CstBridge {
        @JavascriptInterface
        public String getBaseUrl() {
            String saved = preferences.getString(KEY_BASE, "");
            String normalized = normalizeServerUrl(saved);
            return normalized == null ? "" : normalized;
        }

        @JavascriptInterface
        public String getToken() {
            return tokenStore.read();
        }

        @JavascriptInterface
        public void setConfig(String baseUrl, String token) {
            String previousBaseUrl = currentBaseUrl();
            String previousAdminToken = tokenStore.read();
            String normalized = normalizeServerUrl(baseUrl);
            if (normalized != null) {
                preferences.edit().putString(KEY_BASE, normalized).apply();
            }
            tokenStore.write(token);
            runOnUiThread(() -> PaymentPushRegistration.refreshAfterConfigurationChange(
                    MainActivity.this,
                    preferences,
                    tokenStore,
                    previousBaseUrl,
                    previousAdminToken));
        }

        @JavascriptInterface
        public void openSettings() {
            runOnUiThread(MainActivity.this::showSettingsDialog);
        }

        @JavascriptInterface
        public void openPaymentSettings() {
            runOnUiThread(() -> PaymentPushRegistration.showConfigurationDialog(MainActivity.this));
        }

        @JavascriptInterface
        public void openGooglePaySettings() {
            runOnUiThread(() -> GooglePaySettings.show(MainActivity.this));
        }

        @JavascriptInterface
        public void showKeyboard() {
            runOnUiThread(() -> showSoftKeyboard(true));
        }

        @JavascriptInterface
        public void hideKeyboard() {
            runOnUiThread(() -> showSoftKeyboard(false));
        }

        @JavascriptInterface
        public void pasteFromClipboard() {
            runOnUiThread(MainActivity.this::pasteIntoFocusedWebContent);
        }

        @JavascriptInterface
        public boolean openExternalHttpsUrl(String rawUrl) {
            if (rawUrl == null || rawUrl.length() > 2_048) {
                return false;
            }
            Uri uri = Uri.parse(rawUrl.trim());
            if (!"https".equalsIgnoreCase(uri.getScheme())
                    || uri.getHost() == null
                    || uri.getHost().trim().isEmpty()
                    || uri.getUserInfo() != null) {
                return false;
            }
            runOnUiThread(() -> openExternalUri(uri));
            return true;
        }

        @JavascriptInterface
        public String consumePaymentHandoff() {
            return consumePendingPaymentHandoff();
        }

        @JavascriptInterface
        public String consumeAutonomousAgentHandoff() {
            return consumePendingAutonomousAgentHandoff();
        }

        @JavascriptInterface
        public void saveBase64File(String filename, String mimeType, String base64Data) {
            if (base64Data == null || base64Data.length() > MAX_BRIDGE_FILE_BASE64_LENGTH) {
                runOnUiThread(() -> Toast.makeText(
                        MainActivity.this,
                        R.string.file_too_large,
                        Toast.LENGTH_LONG).show());
                return;
            }
            try {
                byte[] decoded = android.util.Base64.decode(base64Data, android.util.Base64.DEFAULT);
                runOnUiThread(() -> startDocumentSave(filename, mimeType, decoded));
            } catch (IllegalArgumentException error) {
                fileSaveFailed(error.getMessage());
            }
        }

        @JavascriptInterface
        public void fileSaveFailed(String details) {
            runOnUiThread(() -> Toast.makeText(
                    MainActivity.this,
                    getString(R.string.download_failed, details),
                    Toast.LENGTH_LONG).show());
        }
    }

    @android.annotation.TargetApi(android.os.Build.VERSION_CODES.R)
    private static class Api30Insets {
        static void showIme(View view) {
            view.getWindowInsetsController().show(WindowInsets.Type.ime());
        }

        static void apply(View view, WindowInsets insets) {
            android.graphics.Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars()
                            | WindowInsets.Type.displayCutout()
                            | WindowInsets.Type.ime());
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom);
        }
    }

    @android.annotation.TargetApi(android.os.Build.VERSION_CODES.TIRAMISU)
    private static class Api33Back {
        static void register(MainActivity activity) {
            activity.getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT,
                    activity::handleBackNavigation);
        }
    }
}
