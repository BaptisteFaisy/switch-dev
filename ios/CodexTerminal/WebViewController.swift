import Foundation
import UIKit
import WebKit

final class WebViewController: UIViewController {
    private enum NativeConfig {
        static let baseURLKey = "codex-switch-terminal.remote.base-url"
        static let bundledBaseURLKey = "CSTServerURL"
        static let vpsRouteMigrationKey = "codex-switch-terminal.vps-route-migrated-20260720"
        static let legacyPCBaseURL = "https://pc-fixe-cst.tail3a8bdf.ts.net"
        static let fallbackBaseURL = "https://azure-duello.tail3a8bdf.ts.net"
        static let configMessage = "cstConfig"
        static let settingsMessage = "cstSettings"
    }

    private var webView: WKWebView!
    private let errorOverlay = UIView()
    private let errorMessage = UILabel()

    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        configureWebView()
        configureErrorOverlay()
        loadConfiguredServer()
    }

    deinit {
        webView?.configuration.userContentController.removeScriptMessageHandler(
            forName: NativeConfig.configMessage
        )
        webView?.configuration.userContentController.removeScriptMessageHandler(
            forName: NativeConfig.settingsMessage
        )
    }

    private func configureWebView() {
        let contentController = WKUserContentController()
        contentController.add(self, name: NativeConfig.configMessage)
        contentController.add(self, name: NativeConfig.settingsMessage)

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .default()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = true
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        configuration.applicationNameForUserAgent = "CodexTerminaliOS/1.0"
        installBridgeScript(in: contentController)

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.translatesAutoresizingMaskIntoConstraints = false
        webView.backgroundColor = .black
        webView.isOpaque = true
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        webView.scrollView.backgroundColor = .black
        webView.scrollView.bounces = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        if #available(iOS 16.4, *) {
            webView.isInspectable = true
        }

        view.addSubview(webView)
        view.keyboardLayoutGuide.followsUndockedKeyboard = false
        NSLayoutConstraint.activate([
            webView.topAnchor.constraint(equalTo: view.topAnchor),
            webView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            webView.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
        ])
        self.webView = webView
    }

    private func configureErrorOverlay() {
        errorOverlay.translatesAutoresizingMaskIntoConstraints = false
        errorOverlay.backgroundColor = UIColor(white: 0.035, alpha: 0.98)
        errorOverlay.isHidden = true

        let title = UILabel()
        title.text = "Serveur indisponible"
        title.textColor = .white
        title.font = .systemFont(ofSize: 22, weight: .bold)
        title.textAlignment = .center

        errorMessage.textColor = UIColor(white: 0.72, alpha: 1)
        errorMessage.font = .systemFont(ofSize: 14)
        errorMessage.numberOfLines = 0
        errorMessage.textAlignment = .center

        let retry = makeButton(title: "Reessayer", primary: true)
        retry.addTarget(self, action: #selector(retryConnection), for: .touchUpInside)
        let settings = makeButton(title: "Configurer", primary: false)
        settings.addTarget(self, action: #selector(openSettings), for: .touchUpInside)

        let actions = UIStackView(arrangedSubviews: [retry, settings])
        actions.axis = .vertical
        actions.spacing = 10

        let stack = UIStackView(arrangedSubviews: [title, errorMessage, actions])
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.axis = .vertical
        stack.spacing = 18
        stack.setCustomSpacing(24, after: errorMessage)

        errorOverlay.addSubview(stack)
        view.addSubview(errorOverlay)
        NSLayoutConstraint.activate([
            errorOverlay.topAnchor.constraint(equalTo: view.topAnchor),
            errorOverlay.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            errorOverlay.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            errorOverlay.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            stack.centerYAnchor.constraint(equalTo: errorOverlay.safeAreaLayoutGuide.centerYAnchor),
            stack.leadingAnchor.constraint(equalTo: errorOverlay.safeAreaLayoutGuide.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: errorOverlay.safeAreaLayoutGuide.trailingAnchor, constant: -28),
            retry.heightAnchor.constraint(greaterThanOrEqualToConstant: 48),
            settings.heightAnchor.constraint(greaterThanOrEqualToConstant: 48),
        ])
    }

    private func makeButton(title: String, primary: Bool) -> UIButton {
        var configuration = UIButton.Configuration.filled()
        configuration.title = title
        configuration.baseForegroundColor = primary ? .black : .white
        configuration.baseBackgroundColor = primary ? .white : UIColor(white: 0.12, alpha: 1)
        configuration.cornerStyle = .medium
        let button = UIButton(configuration: configuration)
        button.titleLabel?.font = .systemFont(ofSize: 16, weight: .semibold)
        return button
    }

    private func installBridgeScript(in contentController: WKUserContentController? = nil) {
        let controller = contentController ?? webView.configuration.userContentController
        controller.removeAllUserScripts()

        let baseURL = javaScriptLiteral(configuredServerURL()?.absoluteString ?? "")
        let token = javaScriptLiteral(KeychainTokenStore.read())
        let source = """
        (() => {
          const state = { baseUrl: \(baseURL), token: \(token) };
          Object.defineProperty(window, "CstIOS", {
            configurable: false,
            enumerable: false,
            value: Object.freeze({
              getBaseUrl: () => state.baseUrl,
              getToken: () => state.token,
              setConfig: (baseUrl, token) => {
                state.baseUrl = String(baseUrl || "").trim();
                state.token = String(token || "").trim();
                window.webkit.messageHandlers.\(NativeConfig.configMessage).postMessage({
                  baseUrl: state.baseUrl,
                  token: state.token
                });
              },
              openSettings: () => {
                window.webkit.messageHandlers.\(NativeConfig.settingsMessage).postMessage(null);
              }
            })
          });
        })();
        """
        controller.addUserScript(
            WKUserScript(source: source, injectionTime: .atDocumentStart, forMainFrameOnly: true)
        )
    }

    private func javaScriptLiteral(_ value: String) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: .fragmentsAllowed),
              let literal = String(data: data, encoding: .utf8)
        else {
            return "\"\""
        }
        return literal
    }

    private func configuredServerURL() -> URL? {
        let defaults = UserDefaults.standard
        let bundled = Bundle.main.object(forInfoDictionaryKey: NativeConfig.bundledBaseURLKey) as? String
        let bundledURL = normalizedServerURL(bundled ?? NativeConfig.fallbackBaseURL)

        if !defaults.bool(forKey: NativeConfig.vpsRouteMigrationKey) {
            let savedURL = defaults.string(forKey: NativeConfig.baseURLKey)
                .flatMap(normalizedServerURL)
            if savedURL?.absoluteString == NativeConfig.legacyPCBaseURL,
               let bundledURL
            {
                defaults.set(bundledURL.absoluteString, forKey: NativeConfig.baseURLKey)
            }
            defaults.set(true, forKey: NativeConfig.vpsRouteMigrationKey)
        }

        let savedURL = defaults.string(forKey: NativeConfig.baseURLKey)
            .flatMap(normalizedServerURL)
        return savedURL ?? bundledURL
    }

    private func normalizedServerURL(_ rawValue: String) -> URL? {
        let trimmed = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              let host = components.host,
              !host.isEmpty,
              components.user == nil,
              components.password == nil,
              scheme == "https" || (scheme == "http" && isLocalHost(host))
        else {
            return nil
        }
        components.fragment = nil
        components.query = nil
        if components.path == "/" {
            components.path = ""
        }
        return components.url
    }

    private func isLocalHost(_ rawHost: String) -> Bool {
        let host = rawHost.lowercased()
        if host == "localhost" || host == "::1" || host.hasSuffix(".local") || !host.contains(".") {
            return true
        }
        if host.contains(":")
            && (host.hasPrefix("fe80:") || host.hasPrefix("fc") || host.hasPrefix("fd"))
        {
            return true
        }

        let octets = host.split(separator: ".").compactMap { Int($0) }
        guard octets.count == 4, octets.allSatisfy({ (0...255).contains($0) }) else {
            return false
        }
        return octets[0] == 10
            || octets[0] == 127
            || (octets[0] == 172 && (16...31).contains(octets[1]))
            || (octets[0] == 192 && octets[1] == 168)
    }

    private func loadConfiguredServer() {
        guard let url = configuredServerURL() else {
            showConnectionError("L'URL configuree est invalide. Utilise HTTPS, ou HTTP uniquement sur le reseau local.")
            return
        }
        errorOverlay.isHidden = true
        webView.load(URLRequest(url: url, cachePolicy: .useProtocolCachePolicy, timeoutInterval: 30))
    }

    private func persist(baseURL: String, token: String) {
        if let normalized = normalizedServerURL(baseURL) {
            UserDefaults.standard.set(normalized.absoluteString, forKey: NativeConfig.baseURLKey)
        }
        KeychainTokenStore.write(token)
    }

    private func showConnectionError(_ details: String) {
        errorMessage.text = details
        errorOverlay.isHidden = false
        view.bringSubviewToFront(errorOverlay)
    }

    @objc private func retryConnection() {
        installBridgeScript()
        loadConfiguredServer()
    }

    @objc private func openSettings() {
        guard presentedViewController == nil else { return }

        let alert = UIAlertController(
            title: "Connexion",
            message: "Le serveur public doit utiliser HTTPS. HTTP est accepte uniquement pour une adresse locale.",
            preferredStyle: .alert
        )
        alert.addTextField { field in
            field.placeholder = "https://serveur.example.com"
            field.text = self.configuredServerURL()?.absoluteString
            field.keyboardType = .URL
            field.textContentType = .URL
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
        }
        alert.addTextField { field in
            field.placeholder = "Token admin"
            field.text = KeychainTokenStore.read()
            field.isSecureTextEntry = true
            field.textContentType = .password
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
        }
        alert.addAction(UIAlertAction(title: "Annuler", style: .cancel))
        alert.addAction(UIAlertAction(title: "Enregistrer", style: .default) { [weak self, weak alert] _ in
            guard let self else { return }
            let baseURL = alert?.textFields?.first?.text ?? ""
            let token = alert?.textFields?.last?.text ?? ""
            guard let normalized = self.normalizedServerURL(baseURL) else {
                self.showConnectionError("URL invalide. Utilise HTTPS, ou une adresse HTTP du reseau local.")
                return
            }
            self.persist(baseURL: normalized.absoluteString, token: token)
            self.installBridgeScript()
            self.loadConfiguredServer()
        })
        present(alert, animated: true)
    }
}

extension WebViewController: WKScriptMessageHandler {
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        switch message.name {
        case NativeConfig.configMessage:
            guard let payload = message.body as? [String: Any] else { return }
            let baseURL = payload["baseUrl"] as? String ?? ""
            let token = payload["token"] as? String ?? ""
            persist(baseURL: baseURL, token: token)
        case NativeConfig.settingsMessage:
            openSettings()
        default:
            break
        }
    }
}

extension WebViewController: WKNavigationDelegate {
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        errorOverlay.isHidden = true
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        handleNavigationError(error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        handleNavigationError(error)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        webView.reload()
    }

    private func handleNavigationError(_ error: Error) {
        let nsError = error as NSError
        guard nsError.code != NSURLErrorCancelled else { return }
        showConnectionError("\(nsError.localizedDescription)\n\nVerifie Tailscale, l'adresse du serveur et ta connexion reseau.")
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.cancel)
            return
        }

        if url.scheme == "about" {
            decisionHandler(.allow)
            return
        }

        let configuredHost = configuredServerURL()?.host?.lowercased()
        let currentHost = webView.url?.host?.lowercased()
        let targetHost = url.host?.lowercased()
        let isInternal = targetHost != nil && (targetHost == configuredHost || targetHost == currentHost)
        if isInternal && navigationAction.targetFrame != nil {
            decisionHandler(.allow)
            return
        }

        if ["http", "https", "mailto", "tel"].contains(url.scheme?.lowercased() ?? "") {
            UIApplication.shared.open(url)
        }
        decisionHandler(.cancel)
    }
}

extension WebViewController: WKUIDelegate {
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if let url = navigationAction.request.url {
            UIApplication.shared.open(url)
        }
        return nil
    }
}
