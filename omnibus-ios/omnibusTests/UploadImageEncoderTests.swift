//  UploadImageEncoderTests.swift
//  The downscale-and-JPEG bound every picked photo passes through.
//
//  The bound is load-bearing twice over: the server's magic-byte sniff
//  rejects HEIC — the common Photos encoding — so re-encoding is what makes a
//  camera-roll pick uploadable at all, and the upload routes cap bodies at
//  10 MiB, which an original easily exceeds.

import Testing
import UIKit

@testable import omnibus

@Suite struct UploadImageEncoderTests {
    /// A solid-color image of an exact pixel size (1x, so points == pixels).
    private func image(width: CGFloat, height: CGFloat) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let size = CGSize(width: width, height: height)
        return UIGraphicsImageRenderer(size: size, format: format).image { context in
            UIColor.systemIndigo.setFill()
            context.fill(CGRect(origin: .zero, size: size))
        }
    }

    private func decodedPixelSize(_ data: Data) -> CGSize? {
        guard let decoded = UIImage(data: data) else { return nil }
        return CGSize(
            width: decoded.size.width * decoded.scale,
            height: decoded.size.height * decoded.scale
        )
    }

    @Test func downscalesTheLongEdgeToTheBoundPreservingAspect() throws {
        let data = try #require(
            UploadImageEncoder.jpeg(image(width: 4000, height: 2000), maxDimension: 1600)
        )
        let size = try #require(decodedPixelSize(data))
        #expect(max(size.width, size.height) == 1600)
        #expect(min(size.width, size.height) == 800)
    }

    @Test func leavesAnImageAlreadyInsideTheBoundUnscaled() throws {
        let data = try #require(
            UploadImageEncoder.jpeg(image(width: 800, height: 1200), maxDimension: 1600)
        )
        let size = try #require(decodedPixelSize(data))
        #expect(size == CGSize(width: 800, height: 1200))
    }

    @Test func emitsJPEGBytesWhateverTheSourceEncoding() throws {
        // Round-trip through PNG so the source is definitively not JPEG.
        let png = try #require(image(width: 100, height: 100).pngData())
        let source = try #require(UIImage(data: png))
        let data = try #require(UploadImageEncoder.jpeg(source, maxDimension: 1600))
        #expect(Array(data.prefix(3)) == [0xFF, 0xD8, 0xFF])
    }

    @Test func refusesAZeroSizedImage() {
        #expect(UploadImageEncoder.jpeg(UIImage(), maxDimension: 1600) == nil)
    }
}
