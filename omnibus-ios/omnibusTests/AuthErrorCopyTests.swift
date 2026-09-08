//  AuthErrorCopyTests.swift
//  What a refused sign-in says, versus what an expired session says.
//
//  Both arrive as a bare 401 with the same body, so the endpoint is the only
//  thing that tells them apart. The client used to map every 401 to "Your
//  session expired. Sign in again." — which, on the sign-in screen itself,
//  reports something that did not happen and sends a reader who mistyped
//  back to retype the same password.

import Testing

@testable import omnibus

@Suite("Auth error copy")
struct AuthErrorCopyTests {
    @Test("a refused sign-in names the credentials, not a session")
    func invalidCredentialsCopyDoesNotClaimAnExpiry() {
        let copy = APIError.invalidCredentials.errorDescription ?? ""
        #expect(copy.contains("username or password"))
        #expect(!copy.lowercased().contains("expired"))
        // Nor the wire status the web form used to print (#2468).
        #expect(!copy.contains("401"))
    }

    @Test("a lockout is not reported as a plain refusal")
    func tooManyAttemptsCopyTellsTheReaderToWait() {
        let copy = APIError.tooManyAttempts.errorDescription ?? ""
        #expect(copy.lowercased().contains("too many"))
        #expect(!copy.contains("429"))
    }

    @Test("an expired session still says so, for every authenticated call")
    func unauthorizedCopyIsUnchanged() {
        #expect(APIError.unauthorized.errorDescription == "Your session expired. Sign in again.")
    }

    @Test("only the two unauthenticated sign-in routes take the credentials mapping")
    func signInPathsAreTheLoginAndRegisterRoutes() {
        #expect(APIClient.isSignInPath("/api/auth/login"))
        #expect(APIClient.isSignInPath("/api/auth/register"))
        // A server mounted under a path prefix still matches.
        #expect(APIClient.isSignInPath("/omnibus/api/auth/login"))
    }

    @Test("an authenticated route keeps the session-expired path")
    func authenticatedPathsAreNotSignIn() {
        #expect(!APIClient.isSignInPath("/api/auth/me"))
        #expect(!APIClient.isSignInPath("/api/auth/logout"))
        #expect(!APIClient.isSignInPath("/api/books"))
        #expect(!APIClient.isSignInPath(nil))
    }
}
