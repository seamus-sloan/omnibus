//! Tests for the shared user-avatar monogram derivation. The photo-vs-letter
//! branch is exercised by the SSR render tests in the sites that mount it.

use super::*;

#[test]
fn initials_uses_first_letters_of_two_tokens() {
    assert_eq!(initials_for("elena koval"), "EK");
    assert_eq!(initials_for("ada.lovelace"), "AL");
    assert_eq!(initials_for("grace_hopper"), "GH");
}

#[test]
fn initials_takes_two_letters_of_single_token() {
    assert_eq!(initials_for("seamus"), "SE");
    assert_eq!(initials_for("a"), "A");
}

#[test]
fn initials_falls_back_for_empty_or_separator_only() {
    assert_eq!(initials_for(""), "?");
    assert_eq!(initials_for("   "), "?");
    assert_eq!(initials_for("..._"), "?");
}

#[test]
fn initials_derive_from_a_display_name_with_punctuation() {
    // A display name is free text, unlike a username — "Alice A." must not
    // produce a trailing-dot token.
    assert_eq!(initials_for("Alice A."), "AA");
    assert_eq!(initials_for("Seamus"), "SE");
}

// SSR render tests — `server`-gated so their contents aren't dead code under
// `web`, where CI lints with `-D warnings`.
#[cfg(feature = "server")]
mod render {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn monogram_harness() -> Element {
        rsx! {
            UserAvatar {
                user_id: 7,
                name: "Alice A.".to_string(),
                has_avatar: false,
                class: "um-initials",
            }
        }
    }

    fn photo_harness() -> Element {
        rsx! {
            UserAvatar {
                user_id: 7,
                name: "Alice A.".to_string(),
                has_avatar: true,
                class: "um-initials",
            }
        }
    }

    #[test]
    fn user_avatar_renders_the_monogram_when_the_user_has_no_picture() {
        let html = render_in_vdom(monogram_harness);
        assert!(html.contains("class=\"um-initials\""));
        assert!(html.contains("AA"));
        assert!(!html.contains("<img"));
        // A monogram is decorative — the name is already in the byline beside
        // it, so a screen reader shouldn't read the initials too.
        assert!(html.contains("aria-hidden"));
    }

    #[test]
    fn user_avatar_renders_the_image_for_a_user_with_a_picture() {
        let html = render_in_vdom(photo_harness);
        assert!(html.contains("/api/users/7/avatar"));
        assert!(html.contains("um-initials-photo"));
        assert!(html.contains("alt=\"Alice A.\""));
        // The photo is inside the same span the monogram would occupy, and
        // that span is not hidden from assistive tech when it holds an image.
        assert!(html.contains("class=\"um-initials\""));
        assert!(!html.contains("aria-hidden"));
    }
}
