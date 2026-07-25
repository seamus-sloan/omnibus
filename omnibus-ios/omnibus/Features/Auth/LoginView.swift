//  LoginView.swift
//  Credential entry with a register toggle. One screen for both modes so the
//  keyboard never has to tear down between them.

import SwiftUI

struct LoginView: View {
    @Environment(AppState.self) private var app
    @Environment(\.palette) private var palette

    private enum Mode { case login, register }
    private enum Field { case username, password }

    @State private var mode: Mode = .login
    @State private var username = ""
    @State private var password = ""
    @State private var isBusy = false
    @State private var error: String?
    @FocusState private var focused: Field?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                header

                VStack(spacing: Spacing.md) {
                    TextField("Username", text: $username)
                        .textFieldStyle(OmnibusFieldStyle())
                        .textContentType(.username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.next)
                        .focused($focused, equals: .username)
                        .onSubmit { focused = .password }

                    SecureField("Password", text: $password)
                        .textFieldStyle(OmnibusFieldStyle())
                        .textContentType(mode == .login ? .password : .newPassword)
                        .submitLabel(.go)
                        .focused($focused, equals: .password)
                        .onSubmit { Task { await submit() } }
                }

                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.ui(13))
                        .foregroundStyle(palette.badColor)
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }

                if mode == .register {
                    Text("Passwords must be at least 10 characters.")
                        .font(.ui(12))
                        .foregroundStyle(palette.ink3Color)
                }

                Button {
                    Task { await submit() }
                } label: {
                    if isBusy {
                        ProgressView().tint(palette.accentInk.color)
                    } else {
                        Text(mode == .login ? "Sign in" : "Create account")
                    }
                }
                .buttonStyle(FilledButtonStyle())
                .disabled(!canSubmit)

                HStack(spacing: 4) {
                    Text(mode == .login ? "No account yet?" : "Already have an account?")
                        .foregroundStyle(palette.ink2Color)
                    Button(mode == .login ? "Register" : "Sign in") {
                        withAnimation(Motion.settle) {
                            mode = mode == .login ? .register : .login
                            error = nil
                        }
                    }
                    .foregroundStyle(palette.accentColor)
                }
                .font(.ui(14))
                .frame(maxWidth: .infinity)

                Button("Use a different server") {
                    Task { await app.changeServer() }
                }
                .font(.ui(13))
                .foregroundStyle(palette.ink3Color)
                .frame(maxWidth: .infinity)

                Spacer(minLength: 0)
            }
            .screenPadding()
            .padding(.top, 72)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(ScreenBackground())
        .animation(Motion.settle, value: error)
        .onAppear { focused = .username }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(mode == .login ? "Welcome back" : "Create an account")
                .font(.display(34))
                .foregroundStyle(palette.ink0Color)
            if let server = app.serverURL {
                Text(server.replacingOccurrences(of: "http://", with: ""))
                    .font(.monoUI(13))
                    .foregroundStyle(palette.ink3Color)
            }
        }
    }

    private var canSubmit: Bool {
        !username.trimmingCharacters(in: .whitespaces).isEmpty
            && !password.isEmpty
            && !isBusy
    }

    private func submit() async {
        guard canSubmit else { return }
        isBusy = true
        error = nil
        defer { isBusy = false }

        do {
            let user = mode == .login
                ? try await AuthService.login(username: username, password: password)
                : try await AuthService.register(username: username, password: password)
            Haptics.success()
            app.signedIn(user)
        } catch {
            Haptics.warning()
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }
}
