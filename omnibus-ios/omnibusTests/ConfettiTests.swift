//  ConfettiTests.swift
//  The confetti model's contract: pieces stay inside the spec's ranges and
//  spread evenly across the container width.

import Foundation
import Testing

@testable import omnibus

/// Deterministic generator so piece assertions are stable run to run.
private struct SeededGenerator: RandomNumberGenerator {
    var state: UInt64
    mutating func next() -> UInt64 {
        // SplitMix64 — tiny and plenty for test determinism.
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}

private func makePieces(count: Int = 40) -> [ConfettiPiece] {
    var rng = SeededGenerator(state: 42)
    return ConfettiPiece.makePieces(count: count, using: &rng)
}

struct ConfettiTests {
    @Test func makePiecesProducesFortyPiecesWithinSpecRanges() {
        let pieces = makePieces()
        #expect(pieces.count == 40)
        for piece in pieces {
            #expect(piece.xFraction >= 0 && piece.xFraction <= 1)
            #expect(piece.delay >= 0 && piece.delay <= 0.5)
            #expect(piece.duration >= 1.5 && piece.duration <= 2.6)
            #expect(piece.size >= 5 && piece.size <= 11)
        }
    }

    @Test func everyFourthPieceIsACircle() {
        let pieces = makePieces()
        for (i, piece) in pieces.enumerated() {
            #expect(piece.isCircle == (i % 4 == 0))
        }
    }

    @Test func piecesSpreadEvenlyAcrossTheWidth() {
        let pieces = makePieces()
        let fractions = pieces.map(\.xFraction)
        #expect(fractions.min() ?? 1 < 0.1)
        #expect(fractions.max() ?? 0 > 0.9)
        // Every tenth of the width holds at least one piece.
        for bucket in 0..<10 {
            let lo = Double(bucket) / 10, hi = lo + 0.1
            #expect(fractions.contains { $0 >= lo && $0 < hi || (bucket == 9 && $0 == 1) })
        }
    }
}
