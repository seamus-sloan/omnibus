//  ProfileDraftTests.swift
//  What the profile sheet's Save actually sends, and when it's enabled.
//
//  The normalization matters twice over: blank means *clear the display name*
//  (reverting every byline to the username), and an unchanged value must not
//  fire a write at all — a no-op POST still renames the wishlist shelf
//  server-side.

import Foundation
import Testing

@testable import omnibus

@Suite struct ProfileDraftTests {
    @Test func seedsTheFieldFromTheExistingDisplayName() {
        let draft = ProfileDraft(original: "Seamus")
        #expect(draft.edited == "Seamus")
        #expect(!draft.hasChanges)
    }

    @Test func seedsAnEmptyFieldWhenNoDisplayNameIsSet() {
        let draft = ProfileDraft(original: nil)
        #expect(draft.edited == "")
        #expect(!draft.hasChanges)
    }

    @Test func trimsSurroundingWhitespaceFromWhatItSends() {
        let draft = ProfileDraft(original: nil, edited: "  Seamus  ")
        #expect(draft.normalizedName == "Seamus")
        #expect(draft.nameChanged)
    }

    @Test func sendsNilForABlankFieldSoTheNameIsCleared() {
        let draft = ProfileDraft(original: "Seamus", edited: "   ")
        #expect(draft.normalizedName == nil)
        #expect(draft.nameChanged)
    }

    @Test func treatsAWhitespaceOnlyEditOfAnAbsentNameAsUnchanged() {
        // Blank over already-absent sends nothing: the write would be a no-op
        // that still re-derives the wishlist shelf name server-side.
        let draft = ProfileDraft(original: nil, edited: "   ")
        #expect(draft.normalizedName == nil)
        #expect(!draft.nameChanged)
        #expect(!draft.hasChanges)
    }

    @Test func treatsARetypedIdenticalNameAsUnchanged() {
        let draft = ProfileDraft(original: "Seamus", edited: "  Seamus  ")
        #expect(!draft.nameChanged)
        #expect(!draft.hasChanges)
    }

    @Test func aPickedImageIsAChangeEvenWithoutARename() {
        let draft = ProfileDraft(original: "Seamus", edited: "Seamus", pickedImage: Data([0x1]))
        #expect(!draft.nameChanged)
        #expect(draft.hasChanges)
    }
}

@Suite struct UserAvatarInitialsTests {
    @Test func usesTheFirstLetterOfTwoTokens() {
        #expect(UserAvatar.initials(for: "Seamus Sloan") == "SS")
        #expect(UserAvatar.initials(for: "ada.lovelace") == "AL")
        #expect(UserAvatar.initials(for: "grace_hopper") == "GH")
    }

    @Test func usesTwoLettersOfASingleToken() {
        #expect(UserAvatar.initials(for: "seamus") == "SE")
        #expect(UserAvatar.initials(for: "a") == "A")
    }

    @Test func fallsBackForAnEmptyOrSeparatorOnlyName() {
        #expect(UserAvatar.initials(for: "") == "?")
        #expect(UserAvatar.initials(for: "   ") == "?")
        #expect(UserAvatar.initials(for: "..._") == "?")
    }
}

@Suite struct UserSummaryDisplayTests {
    private func user(displayName: String?) -> UserSummary {
        UserSummary(
            id: 1, username: "cool-guy-7", isAdmin: false, canUpload: true,
            canEdit: true, canDownload: true, kindleEmail: nil,
            displayName: displayName, hasAvatar: nil
        )
    }

    @Test func prefersTheDisplayNameWhenSet() {
        #expect(user(displayName: "Seamus").display == "Seamus")
    }

    @Test func fallsBackToTheUsername() {
        #expect(user(displayName: nil).display == "cool-guy-7")
    }

    @Test func decodesAPayloadWithoutTheProfileFields() throws {
        // A `CacheKey.me` blob written before this release has neither key;
        // it must still decode rather than signing the reader out.
        let json = """
            {"id":1,"username":"alice","is_admin":false,"can_upload":true,
             "can_edit":true,"can_download":true}
            """
        let decoded = try JSONDecoder().decode(UserSummary.self, from: Data(json.utf8))
        #expect(decoded.displayName == nil)
        #expect(decoded.hasAvatar == nil)
        #expect(decoded.display == "alice")
    }
}
