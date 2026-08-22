//  MultipartBody.swift
//  `multipart/form-data` encoding for the upload endpoints.
//
//  Split from `APIClient` so the wire shape a handler parses — which text
//  fields are present, and in which part — is assertable in a unit test
//  without a server. The upload commit handlers reject a body whose `title`
//  and `author` parts are missing, and that is exactly the kind of omission
//  no amount of client-side type checking catches.

import Foundation

/// One file part of a multipart body.
struct MultipartFile: Equatable, Sendable {
    /// The form field name. Every upload endpoint reads its file parts from
    /// `file`; the avatar route uses `avatar`.
    var fieldName: String = "file"
    var fileName: String
    var mimeType: String
    var data: Data
}

enum MultipartBody {
    /// A boundary token that cannot occur in the payload it delimits.
    static func makeBoundary() -> String { "omnibus.\(UUID().uuidString)" }

    /// Escape a name for a quoted-string `Content-Disposition` parameter.
    ///
    /// Only `/` and NUL are illegal in a POSIX filename, so a perfectly
    /// ordinary book can be called `He said "hi".epub` — and interpolating that
    /// raw closes the quoted string early and corrupts every header after it.
    /// A CR or LF would end the header line outright. RFC 7578 §4.2 prescribes
    /// percent-encoding for exactly this.
    ///
    /// Only those three characters are escaped, and deliberately not `%`
    /// itself: the server does not percent-decode the name, it slugifies it
    /// (a probe upload sent `50%25 off.mp3` and got `50-25-off.mp3` on disk),
    /// so pre-encoding `%` would stamp a literal `25` into the filename of
    /// any ordinary book called something like `50% off`. Escaping only what
    /// can actually break the wire format costs nothing and mangles nothing.
    static func escapeHeaderParameter(_ value: String) -> String {
        value
            .replacingOccurrences(of: "\"", with: "%22")
            .replacingOccurrences(of: "\r", with: "%0D")
            .replacingOccurrences(of: "\n", with: "%0A")
    }

    /// Encode `fields` then `files` into one body.
    ///
    /// Text fields are an ordered array, not a dictionary: `Dictionary`
    /// iteration order varies per process, which would make the request body —
    /// and any test asserting on it — nondeterministic.
    static func encode(
        boundary: String,
        fields: [(name: String, value: String)],
        files: [MultipartFile]
    ) -> Data {
        var body = Data()
        // Data grows geometrically, so appending a multi-part audiobook without
        // this re-copies the whole prefix several times and transiently holds
        // old buffer + new one — enough on its own to jetsam a large upload.
        body.reserveCapacity(files.reduce(4096) { $0 + $1.data.count + 256 })
        for field in fields {
            body.append("--\(boundary)\r\n")
            let name = escapeHeaderParameter(field.name)
            body.append("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n")
            body.append("\(field.value)\r\n")
        }
        for file in files {
            body.append("--\(boundary)\r\n")
            let fieldName = escapeHeaderParameter(file.fieldName)
            let fileName = escapeHeaderParameter(file.fileName)
            body.append(
                "Content-Disposition: form-data; name=\"\(fieldName)\";"
                    + " filename=\"\(fileName)\"\r\n"
            )
            body.append("Content-Type: \(file.mimeType)\r\n\r\n")
            body.append(file.data)
            body.append("\r\n")
        }
        body.append("--\(boundary)--\r\n")
        return body
    }
}

extension Data {
    mutating func append(_ string: String) {
        append(Data(string.utf8))
    }
}
