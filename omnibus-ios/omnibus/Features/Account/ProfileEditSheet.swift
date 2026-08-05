//  ProfileEditSheet.swift
//  Edit the display name and profile picture, from the You tab's identity card.
//
//  Both writes are account configuration, so they call the API directly and
//  report failure inline — never queued for replay (rule 08). Save is disabled
//  while offline rather than failing after the fact.

import PhotosUI
import SwiftUI

/// What the sheet has been changed to, separated from the view so the "what
/// does Save actually send" rules are testable without a UI.
struct ProfileDraft: Equatable {
    /// The name as originally loaded — `nil` when the user has no display name.
    var original: String?
    /// The current field contents.
    var edited: String
    /// Image bytes the user picked this session, if any.
    var pickedImage: Data?

    init(original: String?, edited: String? = nil, pickedImage: Data? = nil) {
        self.original = original
        self.edited = edited ?? original ?? ""
        self.pickedImage = pickedImage
    }

    /// The value a save would send: trimmed, with blank normalized to `nil`
    /// (which clears the display name and reverts attribution to the username).
    var normalizedName: String? {
        let trimmed = edited.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// Whether the name differs from what was loaded. Whitespace-only edits
    /// and a blank field over an already-absent name both count as unchanged,
    /// so Save doesn't fire a write that would change nothing.
    var nameChanged: Bool { normalizedName != original }

    /// Whether Save has anything to do at all.
    var hasChanges: Bool { nameChanged || pickedImage != nil }
}

struct ProfileEditSheet: View {
    let user: UserSummary
    /// Called after a successful save so the caller can refresh its identity.
    var onSaved: () async -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var draft: ProfileDraft
    @State private var photoItem: PhotosPickerItem?
    @State private var saving = false
    @State private var error: String?
    @State private var removedAvatar = false
    private var connectivity = Connectivity.shared

    init(user: UserSummary, onSaved: @escaping () async -> Void) {
        self.user = user
        self.onSaved = onSaved
        _draft = State(initialValue: ProfileDraft(original: user.displayName))
    }

    private var showsAvatar: Bool {
        draft.pickedImage != nil || (user.hasAvatar == true && !removedAvatar)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    avatarSection

                    Plate {
                        PlateField(
                            label: "Display name",
                            text: $draft.edited,
                            isEdited: draft.nameChanged,
                            isFirst: true,
                            hint: user.username
                        )
                    }

                    Text(
                        "Shown on your journals, ratings and shelves. Leave it empty to use your username."
                    )
                    .font(.ui(12.5))
                    .foregroundStyle(palette.ink3Color)

                    if let error {
                        Text(error)
                            .font(.ui(13))
                            .foregroundStyle(palette.badColor)
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 40)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .background(ScreenBackground())
            .navigationTitle("Edit profile")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { Task { await save() } }
                        .disabled(saving || !draft.hasChanges || !connectivity.isOnline)
                }
            }
        }
        .onChange(of: photoItem) { _, item in
            Task { await loadPicked(item) }
        }
        .presentationDetents([.medium, .large])
    }

    private var avatarSection: some View {
        HStack(spacing: Spacing.lg) {
            Group {
                if let data = draft.pickedImage, let image = UIImage(data: data) {
                    Image(uiImage: image).resizable().scaledToFill()
                } else if showsAvatar {
                    UserAvatar(
                        id: user.id, name: draft.normalizedName ?? user.username,
                        hasAvatar: true, size: 72
                    )
                } else {
                    UserAvatar(
                        id: user.id, name: draft.normalizedName ?? user.username,
                        hasAvatar: false, size: 72
                    )
                }
            }
            .frame(width: 72, height: 72)
            .clipShape(Circle())

            VStack(alignment: .leading, spacing: 8) {
                PhotosPicker(selection: $photoItem, matching: .images) {
                    Text("Change picture").font(.ui(14, weight: .medium))
                }
                .disabled(saving || !connectivity.isOnline)

                if showsAvatar {
                    Button("Remove") { Task { await removeAvatar() } }
                        .font(.ui(14))
                        .foregroundStyle(palette.ink2Color)
                        .disabled(saving || !connectivity.isOnline)
                }
            }

            Spacer(minLength: 0)
        }
    }

    /// Load the picked photo and re-encode it as a bounded JPEG. The server
    /// caps an upload at 10 MB and a modern camera roll exceeds that easily,
    /// so downscaling here is what keeps a normal photo uploadable.
    private func loadPicked(_ item: PhotosPickerItem?) async {
        guard let item else { return }
        guard let raw = try? await item.loadTransferable(type: Data.self),
            let image = UIImage(data: raw)
        else {
            error = "That image couldn't be read."
            return
        }
        error = nil
        draft.pickedImage = Self.encodeAvatar(image)
        removedAvatar = false
    }

    /// Downscale to at most 512pt on the long edge and JPEG-encode. An avatar
    /// is never drawn larger than ~72pt, so anything bigger is bytes nobody
    /// sees.
    static func encodeAvatar(_ image: UIImage, maxDimension: CGFloat = 512) -> Data? {
        let longest = max(image.size.width, image.size.height)
        let scale = longest > maxDimension ? maxDimension / longest : 1
        let target = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let renderer = UIGraphicsImageRenderer(size: target)
        let resized = renderer.image { _ in image.draw(in: CGRect(origin: .zero, size: target)) }
        return resized.jpegData(compressionQuality: 0.85)
    }

    private func save() async {
        saving = true
        error = nil
        do {
            if let data = draft.pickedImage {
                try await AuthService.uploadAvatar(data, userID: user.id)
            }
            if draft.nameChanged {
                try await AuthService.setDisplayName(draft.normalizedName)
            }
            await onSaved()
            dismiss()
        } catch {
            // Rule 08: a rejected account-configuration write fails visibly.
            // Never `try?` — a silently-dropped save is a profile the reader
            // believes they changed.
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        saving = false
    }

    private func removeAvatar() async {
        saving = true
        error = nil
        do {
            try await AuthService.deleteAvatar(userID: user.id)
            draft.pickedImage = nil
            removedAvatar = true
            await onSaved()
        } catch {
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        saving = false
    }
}
