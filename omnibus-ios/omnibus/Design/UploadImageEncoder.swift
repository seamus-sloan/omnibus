//  UploadImageEncoder.swift
//  Bounded JPEG encoding for picked photos headed to the server.
//
//  Shared by the avatar and cover upload paths. Photos assets are commonly
//  HEIC — which the server's magic-byte sniff rejects — and camera-roll
//  originals routinely exceed the upload routes' 10 MiB cap, so every picked
//  image is downscaled and re-encoded before it goes on the wire.

import UIKit

enum UploadImageEncoder {
    /// Downscale to at most `maxDimension` pixels on the long edge and
    /// JPEG-encode. Rendered at 1x deliberately, so the bound means pixels
    /// rather than points — the default renderer format multiplies by the
    /// device's screen scale, which would triple the upload on a 3x phone.
    static func jpeg(
        _ image: UIImage, maxDimension: CGFloat, quality: CGFloat = 0.85
    ) -> Data? {
        let longest = max(image.size.width, image.size.height) * image.scale
        guard longest > 0 else { return nil }
        let scale = longest > maxDimension ? maxDimension / longest : 1
        let target = CGSize(
            width: image.size.width * image.scale * scale,
            height: image.size.height * image.scale * scale
        )
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let renderer = UIGraphicsImageRenderer(size: target, format: format)
        let resized = renderer.image { _ in
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        return resized.jpegData(compressionQuality: quality)
    }
}
