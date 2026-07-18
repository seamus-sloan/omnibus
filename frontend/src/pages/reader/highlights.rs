//! Highlight-creation action shared by the selection popover's swatch,
//! Note, and Quote buttons: optimistically annotates the viewer, persists
//! the highlight, and (per `PostCreate`) opens the note composer or quote
//! panel on the created row. Extracted from `BookReadPage`.

use dioxus::prelude::*;
use omnibus_shared::{Highlight, HighlightColor};

use crate::data;

/// What to open on the created highlight after the swatch/Note/Quote actions.
#[derive(Clone, Copy)]
pub(crate) enum PostCreate {
    None,
    Note,
    Quote,
}

/// One highlight-creation request: the selection range plus what to open on
/// the created row afterwards.
pub(crate) struct NewHighlight {
    pub cfi: String,
    pub color: HighlightColor,
    pub text: String,
    pub post: PostCreate,
}

/// Where a created highlight lands: the list signal plus the note/quote
/// panel targets [`PostCreate`] may open.
#[derive(Clone, Copy)]
pub(crate) struct HighlightTargets {
    pub highlights: Signal<Vec<Highlight>>,
    pub note_target: Signal<Option<Highlight>>,
    pub quote_target: Signal<Option<Highlight>>,
}

/// Optimistically annotate the selection, persist the highlight, and — per
/// `req.post` — open the note composer or quote panel on the created row.
/// Shared by the swatch (highlight), Note, and Quote popover actions. Rolls
/// the optimistic annotation back if the write fails.
pub(crate) fn spawn_create_highlight(
    server_url: String,
    uuid: String,
    req: NewHighlight,
    targets: HighlightTargets,
) {
    let NewHighlight {
        cfi,
        color,
        text,
        post,
    } = req;
    let HighlightTargets {
        mut highlights,
        mut note_target,
        mut quote_target,
    } = targets;
    #[cfg(any(feature = "web", feature = "mobile"))]
    super::reader_call_json2("addAnnotation", &cfi, color.as_str());
    let create = omnibus_shared::CreateHighlight {
        book_uuid: uuid,
        epub_cfi_range: cfi.clone(),
        color,
        text: if text.is_empty() { None } else { Some(text) },
    };
    spawn(async move {
        match data::create_highlight(&server_url, create).await {
            Ok(h) => {
                highlights.write().push(h.clone());
                match post {
                    PostCreate::Note => note_target.set(Some(h)),
                    PostCreate::Quote => quote_target.set(Some(h)),
                    PostCreate::None => {}
                }
            }
            Err(_) => {
                #[cfg(any(feature = "web", feature = "mobile"))]
                super::reader_call_json("removeAnnotation", &cfi);
                let _ = &cfi;
            }
        }
    });
}
