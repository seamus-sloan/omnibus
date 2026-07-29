//  ServerConnectView.swift
//  Pre-login server URL entry. Probes `/api/_health` before saving so a typo
//  reads as "can't reach that" rather than surfacing later as a login failure.

import SwiftUI

struct ServerConnectView: View {
    @Environment(AppState.self) private var app
    @Environment(\.palette) private var palette

    @State private var address = ""
    @State private var isProbing = false
    @State private var error: String?
    @FocusState private var focused: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                header

                VStack(alignment: .leading, spacing: Spacing.sm) {
                    Text("Server address")
                        .font(.ui(13, weight: .medium))
                        .foregroundStyle(palette.ink2Color)

                    TextField("192.168.1.10:3000", text: $address)
                        // Stable handle for omnibusUITests — placeholder text
                        // and layout can change; this shouldn't.
                        .accessibilityIdentifier("server-address")
                        .textFieldStyle(OmnibusFieldStyle())
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.go)
                        .focused($focused)
                        .onSubmit { Task { await connect() } }

                    Text("Include the port if it isn't 80 or 443. `https://` is assumed only if you type it.")
                        .font(.ui(12))
                        .foregroundStyle(palette.ink3Color)
                }

                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.ui(13))
                        .foregroundStyle(palette.badColor)
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }

                Button {
                    Task { await connect() }
                } label: {
                    if isProbing {
                        ProgressView().tint(palette.accentInk.color)
                    } else {
                        Text("Connect")
                    }
                }
                .buttonStyle(FilledButtonStyle())
                .disabled(address.trimmingCharacters(in: .whitespaces).isEmpty || isProbing)

                Spacer(minLength: 0)
            }
            .screenPadding()
            .padding(.top, 72)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(ScreenBackground())
        .animation(Motion.settle, value: error)
        .onAppear { focused = true }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            OmnibusMark(height: 36)
                .padding(.bottom, Spacing.xs)
            Text("Connect to Omnibus")
                .font(.display(34))
                .foregroundStyle(palette.ink0Color)
            Text("Point the app at your self-hosted library server.")
                .font(.ui(15))
                .foregroundStyle(palette.ink2Color)
        }
    }

    private func connect() async {
        guard let normalized = ServerURLStore.normalize(address) else {
            error = "That doesn't look like a valid address."
            return
        }
        isProbing = true
        error = nil
        defer { isProbing = false }

        switch await AuthService.probe(serverURL: normalized) {
        case .success:
            Haptics.success()
            await app.setServer(normalized)
        case let .failure(failure):
            Haptics.warning()
            error = "Couldn't reach \(normalized). \(failure.errorDescription ?? "")"
        }
    }
}

/// Shared text-field chrome — a filled, rounded field that reads as a native
/// control but carries the Atrium palette.
struct OmnibusFieldStyle: TextFieldStyle {
    @Environment(\.palette) private var palette

    func _body(configuration: TextField<Self._Label>) -> some View {
        configuration
            .font(.ui(16))
            .foregroundStyle(palette.ink0Color)
            .padding(.horizontal, 14)
            .padding(.vertical, 13)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .strokeBorder(palette.lineColor, lineWidth: 0.5)
            )
    }
}
