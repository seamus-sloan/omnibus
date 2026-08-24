//! Stop 06 · The files — every way you hold this book: the per-format rows
//! with their read/listen/send/download actions, the physical copies +
//! wishlist slot, the metadata mini-table, and the edit / merge / delete
//! actions.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::{BookActionMeta, FormatSwitcher};
use crate::Route;

use super::body::{bd_identifier_key, bd_identifier_label};
use super::physical::{BdBookIdentity, BdPhysicalPanel, BdWishlistRailSlot};
use super::w4::{W4AdminActions, W4ViewFacts};
use super::{BdMetaRow, PhysSignals};

/// The Files stop.
#[component]
pub(super) fn W4FilesStop(
    b: EbookMetadata,
    view: W4ViewFacts,
    admin: W4AdminActions,
    phys: PhysSignals,
    refresh: Signal<u32>,
    is_fileless: bool,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    rsx! {
        div { class: "bdw4-k", "Every way you hold this book" }
        // One list, one row per way you hold the book — file formats first,
        // then the physical copies and the wishlist. The design has no
        // separate badge row or physical panel here: a copy row *is* the
        // statement that you hold it that way.
        div { class: "bdw4-copies rx-copies",
            if !b.formats.is_empty() {
                FormatSwitcher {
                    w4: true,
                    formats: b.formats.clone(),
                    meta: BookActionMeta {
                        uuid: uuid.clone(),
                        author: b.creators.first().map(|c| c.name.clone()).unwrap_or_default(),
                        title: view.title.clone(),
                        epub_size_bytes: b.epub_size_bytes,
                        book_files: b.book_files.clone(),
                    },
                }
            }
            BdPhysicalPanel {
                uuid: uuid.clone(),
                is_fileless,
                refresh,
                phys,
                w4: true,
            }
            BdWishlistRailSlot {
                identity: BdBookIdentity {
                    uuid: uuid.clone(),
                    has_physical: b.has_physical,
                    isbn: b.isbn13.clone(),
                    title: view.title.clone(),
                    author: view.primary_author.clone(),
                },
                phys,
                w4: true,
            }
        }
        div { class: "divider bdw4-files-div" }
        table { class: "rx-kv bd-meta-table mono",
            tbody {
                BdMetaRow { k: "Title".to_string(), v: view.title.clone() }
                if !view.authors_line.is_empty() {
                    BdMetaRow { k: "Author".to_string(), v: view.authors_line.clone() }
                }
                if let Some(p) = b.publisher.clone() { BdMetaRow { k: "Publisher".to_string(), v: p } }
                if let Some(d) = b.published.clone() { BdMetaRow { k: "Published".to_string(), v: d } }
                if let Some(l) = b.language.clone() { BdMetaRow { k: "Language".to_string(), v: l } }
                for ident in b.identifiers.iter() {
                    BdMetaRow {
                        key: "{bd_identifier_key(ident)}",
                        k: bd_identifier_label(ident),
                        v: ident.value.clone(),
                    }
                }
                if let Some(added) = b.added_at.as_deref() {
                    // RFC 3339 timestamp → its date part; the clock is noise here.
                    BdMetaRow {
                        k: "Added".to_string(),
                        v: added.get(0..10).unwrap_or(added).to_string(),
                    }
                }
            }
        }
        div { class: "bdw4-files-actions",
            Link {
                to: Route::MetadataEdit { uuid: uuid.clone() },
                class: "btn ghost sm",
                "data-testid": "edit-metadata",
                "Edit metadata\u{2026}"
            }
            {admin.merge_button}
            {admin.delete_button}
        }
    }
}
