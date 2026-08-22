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
        for field in fields {
            body.append("--\(boundary)\r\n")
            body.append("Content-Disposition: form-data; name=\"\(field.name)\"\r\n\r\n")
            body.append("\(field.value)\r\n")
        }
        for file in files {
            body.append("--\(boundary)\r\n")
            body.append(
                "Content-Disposition: form-data; name=\"\(file.fieldName)\";"
                    + " filename=\"\(file.fileName)\"\r\n"
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
