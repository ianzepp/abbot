import SwiftUI

/// First-launch setup view: provider picker, API key, model selection.
struct SetupView: View {
    @Binding var isSetupComplete: Bool

    @State private var selectedProvider = "anthropic"
    @State private var apiKey = ""
    @State private var model = ""
    @State private var errorMessage: String?
    @State private var isSubmitting = false

    private var currentProvider: (id: String, name: String, baseURL: String, keyEnv: String, defaultModel: String)? {
        ConfigWriter.providers.first { $0.id == selectedProvider }
    }

    private var needsApiKey: Bool {
        currentProvider?.keyEnv.isEmpty == false
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            VStack(spacing: 8) {
                Text("Welcome to Abbot")
                    .font(.largeTitle)
                    .fontWeight(.bold)

                Text("Configure your LLM provider to get started.")
                    .font(.body)
                    .foregroundColor(.secondary)
            }
            .padding(.top, 40)
            .padding(.bottom, 32)

            // Form
            VStack(alignment: .leading, spacing: 20) {
                // Provider picker
                VStack(alignment: .leading, spacing: 6) {
                    Text("Provider")
                        .font(.headline)

                    Picker("", selection: $selectedProvider) {
                        ForEach(ConfigWriter.providers, id: \.id) { provider in
                            Text(provider.name).tag(provider.id)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .onChange(of: selectedProvider) { _ in
                        model = currentProvider?.defaultModel ?? ""
                    }
                }

                // API Key
                if needsApiKey {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("API Key")
                            .font(.headline)

                        SecureField("Enter your \(currentProvider?.name ?? "") API key", text: $apiKey)
                            .textFieldStyle(.roundedBorder)
                    }
                }

                // Model
                VStack(alignment: .leading, spacing: 6) {
                    Text("Model")
                        .font(.headline)

                    TextField("Model identifier", text: $model)
                        .textFieldStyle(.roundedBorder)
                }

                // Error
                if let errorMessage = errorMessage {
                    Text(errorMessage)
                        .foregroundColor(.red)
                        .font(.caption)
                }
            }
            .padding(.horizontal, 40)

            Spacer()

            // Submit button
            Button(action: submit) {
                if isSubmitting {
                    ProgressView()
                        .controlSize(.small)
                        .padding(.horizontal, 20)
                } else {
                    Text("Get Started")
                        .frame(minWidth: 120)
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(isSubmitting || (needsApiKey && apiKey.isEmpty) || model.isEmpty)
            .padding(.bottom, 40)
        }
        .frame(width: 480, height: 460)
        .onAppear {
            model = currentProvider?.defaultModel ?? ""
        }
    }

    private func submit() {
        isSubmitting = true
        errorMessage = nil

        let config = ConfigWriter.SetupConfig(
            provider: selectedProvider,
            apiKey: apiKey,
            model: model
        )

        do {
            try ConfigWriter.write(config: config)
            isSetupComplete = true
        } catch {
            errorMessage = "Failed to write configuration: \(error.localizedDescription)"
        }

        isSubmitting = false
    }
}
