//  APIClient.swift
//  Bearer-authenticated transport for the `/api/*` REST surface.
//
//  Mirrors `frontend/src/data/`: every request carries the stored bearer
//  token, a 401 clears it and flips the app back to the login screen, and a
//  transport failure marks the client offline so the cache layer can serve
//  reads without waiting on a timeout.

import Foundation

enum APIError: LocalizedError {
    case notConfigured
    case offline
    case unauthorized
    case http(status: Int, message: String)
    case decoding(String)
    case transport(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: "No server configured."
        case .offline: "You're offline."
        case .unauthorized: "Your session expired. Sign in again."
        case let .http(status, message):
            message.isEmpty ? "Server error (\(status))." : message
        case let .decoding(detail): "Unexpected response from the server. \(detail)"
        case let .transport(detail): detail
        }
    }

    /// Whether the failure is one a cached read should transparently absorb.
    var isRecoverableOffline: Bool {
        switch self {
        case .offline, .transport: true
        default: false
        }
    }
}

/// Emitted so the root view can react without every call site plumbing it.
extension Notification.Name {
    static let omnibusUnauthorized = Notification.Name("omnibus.unauthorized")
    static let omnibusConnectivityChanged = Notification.Name("omnibus.connectivity")
}

actor APIClient {
    static let shared = APIClient()

    private var baseURL: String?
    private var token: String?
    private(set) var isOnline = true

    private let session: URLSession
    private let encoder: JSONEncoder = {
        let e = JSONEncoder()
        return e
    }()
    private let decoder: JSONDecoder = {
        let d = JSONDecoder()
        return d
    }()

    init() {
        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = false
        config.timeoutIntervalForRequest = 20
        config.timeoutIntervalForResource = 120
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        session = URLSession(configuration: config)
        baseURL = ServerURLStore.load()
        token = TokenStore.load()
    }

    // MARK: - Configuration

    func configure(baseURL: String?) {
        self.baseURL = baseURL
    }

    func setToken(_ token: String?) {
        self.token = token
        if let token {
            TokenStore.save(token)
        } else {
            TokenStore.clear()
        }
    }

    func currentBaseURL() -> String? { baseURL }
    func hasToken() -> Bool { token != nil }

    /// Absolute URL for a server-relative path (covers, audio parts, EPUB
    /// files). Returns `nil` before a server is configured.
    func absoluteURL(_ path: String) -> URL? {
        guard let baseURL else { return nil }
        if path.hasPrefix("http://") || path.hasPrefix("https://") { return URL(string: path) }
        return URL(string: baseURL + (path.hasPrefix("/") ? path : "/" + path))
    }

    /// Header set for direct `AVPlayer` / `WKWebView` loads that bypass this
    /// client but still need the bearer token.
    func authHeaders() -> [String: String] {
        guard let token else { return [:] }
        return ["Authorization": "Bearer \(token)"]
    }

    // MARK: - Requests

    func get<T: Decodable>(_ path: String, query: [String: String?] = [:]) async throws -> T {
        try await send(path: path, method: "GET", query: query, body: Optional<Never>.none)
    }

    @discardableResult
    func post<Body: Encodable, T: Decodable>(_ path: String, body: Body) async throws -> T {
        try await send(path: path, method: "POST", body: body)
    }

    @discardableResult
    func put<Body: Encodable, T: Decodable>(_ path: String, body: Body) async throws -> T {
        try await send(path: path, method: "PUT", body: body)
    }

    @discardableResult
    func patch<Body: Encodable, T: Decodable>(_ path: String, body: Body) async throws -> T {
        try await send(path: path, method: "PATCH", body: body)
    }

    @discardableResult
    func delete<T: Decodable>(_ path: String) async throws -> T {
        try await send(path: path, method: "DELETE", body: Optional<Never>.none)
    }

    /// Raw bytes for a server path — covers, EPUB files, audio parts.
    func data(for path: String) async throws -> Data {
        guard let url = absoluteURL(path) else { throw APIError.notConfigured }
        var request = URLRequest(url: url)
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        do {
            let (data, response) = try await session.data(for: request)
            try validate(response, data: data)
            markOnline(true)
            return data
        } catch let error as APIError {
            throw error
        } catch {
            markOnline(false)
            throw APIError.transport(error.localizedDescription)
        }
    }

    /// Multipart upload used by the cover, photo, and book-upload endpoints.
    func upload<T: Decodable>(
        _ path: String,
        fileData: Data,
        fileName: String,
        fieldName: String = "file",
        mimeType: String = "application/octet-stream",
        fields: [String: String] = [:]
    ) async throws -> T {
        guard let url = absoluteURL(path) else { throw APIError.notConfigured }
        let boundary = "omnibus.\(UUID().uuidString)"
        var body = Data()
        for (key, value) in fields {
            body.append("--\(boundary)\r\n")
            body.append("Content-Disposition: form-data; name=\"\(key)\"\r\n\r\n")
            body.append("\(value)\r\n")
        }
        body.append("--\(boundary)\r\n")
        body.append(
            "Content-Disposition: form-data; name=\"\(fieldName)\"; filename=\"\(fileName)\"\r\n"
        )
        body.append("Content-Type: \(mimeType)\r\n\r\n")
        body.append(fileData)
        body.append("\r\n--\(boundary)--\r\n")

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        request.httpBody = body
        request.timeoutInterval = 300

        do {
            let (data, response) = try await session.data(for: request)
            try validate(response, data: data)
            markOnline(true)
            return try decode(data)
        } catch let error as APIError {
            throw error
        } catch {
            markOnline(false)
            throw APIError.transport(error.localizedDescription)
        }
    }

    /// `GET` that also surfaces the `X-Next-Cursor` header the paginated
    /// library read uses for keyset pagination.
    func getPaged<T: Decodable>(
        _ path: String,
        query: [String: String?] = [:]
    ) async throws -> (value: T, nextCursor: String?) {
        let request = try buildRequest(path: path, method: "GET", query: query, body: Optional<Never>.none)
        do {
            let (data, response) = try await session.data(for: request)
            try validate(response, data: data)
            markOnline(true)
            let cursor = (response as? HTTPURLResponse)?
                .value(forHTTPHeaderField: "X-Next-Cursor")?.nilIfBlank
            return (try decode(data), cursor)
        } catch let error as APIError {
            throw error
        } catch {
            markOnline(false)
            throw APIError.transport(error.localizedDescription)
        }
    }

    // MARK: - Internals

    private func send<Body: Encodable, T: Decodable>(
        path: String,
        method: String,
        query: [String: String?] = [:],
        body: Body?
    ) async throws -> T {
        let request = try buildRequest(path: path, method: method, query: query, body: body)
        do {
            let (data, response) = try await session.data(for: request)
            try validate(response, data: data)
            markOnline(true)
            return try decode(data)
        } catch let error as APIError {
            throw error
        } catch {
            markOnline(false)
            throw APIError.transport(error.localizedDescription)
        }
    }

    private func buildRequest<Body: Encodable>(
        path: String,
        method: String,
        query: [String: String?],
        body: Body?
    ) throws -> URLRequest {
        guard let baseURL else { throw APIError.notConfigured }
        guard var components = URLComponents(string: baseURL + path) else {
            throw APIError.notConfigured
        }
        let items = query.compactMap { key, value in
            value.map { URLQueryItem(name: key, value: $0) }
        }
        if !items.isEmpty { components.queryItems = items }
        guard let url = components.url else { throw APIError.notConfigured }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try encoder.encode(body)
        }
        return request
    }

    private func validate(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { return }
        guard !(200..<300).contains(http.statusCode) else { return }
        if http.statusCode == 401 {
            token = nil
            TokenStore.clear()
            Task { @MainActor in
                NotificationCenter.default.post(name: .omnibusUnauthorized, object: nil)
            }
            throw APIError.unauthorized
        }
        throw APIError.http(status: http.statusCode, message: Self.errorMessage(from: data))
    }

    private func decode<T: Decodable>(_ data: Data) throws -> T {
        if T.self == Empty.self { return Empty() as! T }
        // Several mutating endpoints answer 200 with no body.
        if data.isEmpty, let empty = Empty() as? T { return empty }
        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            throw APIError.decoding(String(describing: error))
        }
    }

    /// Handlers answer either a bare string or `{"error": "..."}`.
    private static func errorMessage(from data: Data) -> String {
        guard !data.isEmpty else { return "" }
        if let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            for key in ["error", "message", "detail"] {
                if let value = object[key] as? String { return value }
            }
        }
        let raw = String(data: data, encoding: .utf8) ?? ""
        return raw.count > 300 ? String(raw.prefix(300)) : raw
    }

    private func markOnline(_ online: Bool) {
        guard isOnline != online else { return }
        isOnline = online
        Task { @MainActor in
            NotificationCenter.default.post(
                name: .omnibusConnectivityChanged, object: nil, userInfo: ["online": online]
            )
        }
    }

    func setOnline(_ online: Bool) {
        markOnline(online)
    }
}

/// Stand-in for endpoints that answer with no meaningful body.
struct Empty: Codable, Sendable {}

private extension Data {
    mutating func append(_ string: String) {
        append(Data(string.utf8))
    }
}
