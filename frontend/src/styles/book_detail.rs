//! Styles for the Book Detail page (`pages::book_detail`) and its
//! supporting bits — cover/meta grid, breadcrumb, description, format
//! switcher rows, tag list, and identifier grid.

/// CSS chunk for the book detail page.
pub const STYLES: &str = r#"
/* ===== Book detail page ===== */
.book-detail {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2rem;
  align-items: start;
}
@media (max-width: 600px) {
  .book-detail { grid-template-columns: 1fr; }
}
.book-detail-cover { width: 220px; max-width: 100%; }
.book-detail-cover img {
  width: 100%; height: auto; display: block;
  border-radius: 6px; box-shadow: 0 4px 20px rgba(0,0,0,.5);
}
.book-detail-cover-fallback {
  width: 220px; height: 300px; background: rgba(255,255,255,.05);
  border-radius: 6px; display: flex; align-items: center;
  justify-content: center; font-size: 3rem; color: rgba(255,255,255,.2);
}
.book-detail-meta { display: flex; flex-direction: column; gap: .5rem; min-width: 0; }
.breadcrumb {
  display: flex; gap: .5rem; align-items: center;
  font-size: .85rem; color: rgba(255,255,255,.5); margin-bottom: .5rem;
}
.breadcrumb a { color: #22d3ee; text-decoration: none; }
.breadcrumb a:hover { text-decoration: underline; }
.book-detail-description { line-height: 1.6; color: rgba(255,255,255,.8); margin: .5rem 0 1rem; }
.book-detail-description > :first-child { margin-top: 0; }
.book-detail-description > :last-child { margin-bottom: 0; }
.book-detail-description p { margin: 0 0 .75rem; }
.book-detail-description ul, .book-detail-description ol { margin: 0 0 .75rem; padding-left: 1.25rem; }
.book-detail-description a { color: #22d3ee; }
.format-switcher {
  display: flex; flex-direction: column; gap: .4rem;
  margin: .75rem 0; padding: .5rem .75rem;
  background: rgba(255,255,255,.03);
  border: 1px solid rgba(255,255,255,.08);
  border-radius: 6px;
}
.format-row {
  display: flex; align-items: center; gap: .75rem; flex-wrap: wrap;
}
.format-row + .format-row {
  padding-top: .4rem; border-top: 1px solid rgba(255,255,255,.05);
}
.format-badge {
  font-family: monospace; font-size: .75rem; font-weight: 600;
  letter-spacing: .05em; padding: .15rem .5rem;
  background: rgba(34,211,238,.12); color: #22d3ee;
  border: 1px solid rgba(34,211,238,.3); border-radius: 4px;
  min-width: 3.5rem; text-align: center;
}
.format-actions { display: flex; gap: .5rem; flex-wrap: wrap; }
.format-actions-empty {
  font-size: .8rem; color: rgba(255,255,255,.4); font-style: italic;
}
.tag-list { display: flex; flex-wrap: wrap; gap: .4rem; list-style: none; padding: 0; margin: .4rem 0; }
.tag {
  background: rgba(34,211,238,.12); border: 1px solid rgba(34,211,238,.3);
  border-radius: 9999px; padding: .2rem .65rem; font-size: .8rem; color: #22d3ee;
}
.identifier-list {
  display: grid; grid-template-columns: auto 1fr;
  gap: .2rem .75rem; font-size: .85rem; margin: .5rem 0;
}
.identifier-list dt { color: rgba(255,255,255,.5); font-family: monospace; }
.identifier-list dd { margin: 0; font-family: monospace; }
.ratings-slot, .suggestions-slot { min-height: 1px; }
"#;
