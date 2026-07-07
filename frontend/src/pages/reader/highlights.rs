//! Highlight-creation action shared by the selection popover's swatch,
//! Note, and Quote buttons: optimistically annotates the viewer, persists
//! the highlight, and (per `PostCreate`) opens the note composer or quote
//! panel on the created row. Extracted from `BookReadPage`.

use dioxus::prelude::*;

use crate::data;

use omnibus_shared::{Highlight, HighlightColor};

/// What to open on the created highlight after the swatch/Note/Quote actions.
#[derive(Clone, Copy)]
pub(crate) enum PostCreate {
    None,
    Note,
    Quote,
}

/// Optimistically annotate the selection, persist the highlight, and — per
/// `post` — open the note composer or quote panel on the created row. Shared
/// by the swatch (highlight), Note, and Quote popover actions. Rolls the
/// optimistic annotation back if the write fails.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_create_highlight(
    uuid: String,
    cfi: String,
    color: HighlightColor,
    text: String,
    mut highlights: Signal<Vec<Highlight>>,
    mut note_target: Signal<Option<Highlight>>,
    mut quote_target: Signal<Option<Highlight>>,
    post: PostCreate,
) {
    #[cfg(feature = "web")]
    super::reader_call_json2("addAnnotation", &cfi, color.as_str());
    let create = omnibus_shared::CreateHighlight {
        book_uuid: uuid,
        epub_cfi_range: cfi.clone(),
        color,
        text: if text.is_empty() { None } else { Some(text) },
    };
    spawn(async move {
        match data::create_highlight("", create).await {
            Ok(h) => {
                highlights.write().push(h.clone());
                match post {
                    PostCreate::Note => note_target.set(Some(h)),
                    PostCreate::Quote => quote_target.set(Some(h)),
                    PostCreate::None => {}
                }
            }
            Err(_) => {
                #[cfg(feature = "web")]
                super::reader_call_json("removeAnnotation", &cfi);
                let _ = &cfi;
            }
        }
    });
}
