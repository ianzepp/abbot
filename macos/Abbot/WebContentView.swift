import SwiftUI
import WebKit

/// A WKWebView wrapper that loads the Abbot web UI from localhost:8080.
/// Retries loading on connection failure until the daemon is ready.
struct WebContentView: NSViewRepresentable {
    let url: URL

    init(url: URL = URL(string: "http://127.0.0.1:8080")!) {
        self.url = url
    }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(url: url)
    }

    class Coordinator: NSObject, WKNavigationDelegate {
        let url: URL
        private var retryCount = 0
        private let maxRetries = 60

        init(url: URL) {
            self.url = url
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            retryLoad(webView: webView)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            retryLoad(webView: webView)
        }

        private func retryLoad(webView: WKWebView) {
            guard retryCount < maxRetries else { return }
            retryCount += 1
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                webView.load(URLRequest(url: self.url))
            }
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            retryCount = 0
        }
    }
}
