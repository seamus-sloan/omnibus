//! Styles for the Landing / library page (`pages::landing`) — both the
//! power-user `<table>` view (with F5.9-lite inline edit affordances)
//! and the F1.7 Atrium cover-grid view (header / toolbar / filter chips
//! / sidebar / grid).

/// CSS chunk for the landing page (cover grid + power-user table).
pub const STYLES: &str = r#"
/* Power-user table — dense rows, mono uppercase headers, hover row background.
   Atrium tokens drive colors so light theme works for free. */
.ebook-table-wrap {
  margin-top: 0.5rem;
  overflow-x: auto;
}
.ebook-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12.5px;
  table-layout: auto;
  color: var(--ink-1);
}
.ebook-table td,
.ebook-table th { white-space: nowrap; }
.ebook-table .ebook-col-title { white-space: normal; }
.ebook-table .ebook-title-cell {
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.ebook-table thead th {
  text-align: left;
  padding: 0.55rem 0.65rem;
  color: var(--ink-2);
  font-family: var(--mono);
  font-weight: 500;
  font-size: 10.5px;
  text-transform: uppercase;
  letter-spacing: 0.14em;
  border-bottom: 1px solid var(--line);
  background: transparent;
  position: sticky;
  top: 0;
}
.ebook-table tbody td {
  padding: 0.55rem 0.65rem;
  border-bottom: 1px solid var(--line-2);
  color: var(--ink-1);
  vertical-align: middle;
}
.ebook-row {
  cursor: pointer;
  transition: background 0.15s;
}
.ebook-row:hover { background: var(--bg-1); }
.ebook-row:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
  background: var(--bg-1);
}
.ebook-row:last-child td { border-bottom: 0; }

.ebook-col-cover { width: 40px; }
.ebook-thumb {
  width: 26px;
  height: 38px;
  object-fit: cover;
  border-radius: 2px;
  display: block;
  background: var(--bg-2);
  box-shadow: 0 4px 8px -4px color-mix(in oklch, black 60%, transparent);
}
.ebook-thumb-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--ink-3);
  font-size: 0.7rem;
}
.ebook-col-title { min-width: 220px; }
.ebook-title-cell {
  color: var(--ink-0);
  font-weight: 500;
}

/* Formats column: mono bordered chips per format. */
.ebook-col-formats { min-width: 90px; }
.ebook-col-formats .format-badge + .format-badge { margin-left: 4px; }
.format-badge {
  display: inline-flex;
  align-items: center;
  font-family: var(--mono);
  font-size: 10px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--ink-1);
  border: 1px solid var(--line-2);
  border-radius: 4px;
  padding: 2px 6px;
  background: transparent;
}
.ebook-cell-formats-empty { color: var(--ink-3); }

/* F5.9-lite inline edit. Admin-only — `.ebook-cell-editable` is added
   by the EditableCell component when `is_admin = true`, so non-admins
   never see the hover/cursor affordance.
   The focused cell carries a thin accent-colored inset outline (via
   box-shadow so it doesn't shift the table's column widths). Hover
   surfaces a quieter ghost of the same outline so admins can spot
   which cells are editable without committing to one. The inner
   `<input>` is borderless and transparent — the cell IS the editor. */
.ebook-cell-editable {
  cursor: text;
  transition: outline 0.1s ease, background 0.1s ease;
}
/* Dashed amber border on hover — matches the F5.9-lite design comp's
   "editable affordance" treatment. `outline` (not `border`) so the
   surrounding column widths stay locked. */
.ebook-cell-editable:hover {
  outline: 1px dashed color-mix(in oklch, var(--accent) 55%, transparent);
  outline-offset: -1px;
}
.ebook-cell-editing,
.ebook-cell-editable.ebook-cell-editing:hover {
  background: var(--bg-0);
  outline: 1px solid var(--accent);
  outline-offset: -1px;
}
.ebook-cell-edit {
  width: 100%;
  box-sizing: border-box;
  font: inherit;
  color: var(--ink-0);
  background: transparent;
  border: none;
  padding: 0;
  outline: none;
}
/* Wraps the ChipEditor inside the Authors cell so the chips + input
   + dropdown all live within the td. Allows chips to wrap; the cell
   grows vertically as needed. The dropdown (`chip-editor-suggestions`)
   anchors via its own `position: relative` wrap. */
.ebook-cell-chip-host {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
  width: 100%;
}
.ebook-cell-chip-host .chip-editor-input-wrap {
  min-width: 80px;
}
/* Author cell allows vertical growth so wrapped chips don't get clipped.
   Hover-state cell shows the same default vertical alignment as the
   read-only rows to avoid a height flash on first hover. */
.ebook-col-author { vertical-align: middle; }
.ebook-edit-row td.ebook-edit-cell {
  background: var(--bg-1);
  padding: 10px 12px;
}
.ebook-edit-bar {
  display: flex;
  align-items: center;
  gap: 12px;
}
.ebook-edit-label {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--ink-3);
  flex-shrink: 0;
}
.ebook-edit-chips {
  flex: 1;
}

@media (max-width: 1100px) {
  .ebook-table .ebook-col-language { display: none; }
}
@media (max-width: 1000px) {
  .ebook-table .ebook-col-formats { display: none; }
}
@media (max-width: 900px) {
  .ebook-table .ebook-col-published { display: none; }
}
@media (max-width: 720px) {
  .ebook-table .ebook-col-publisher { display: none; }
}
@media (max-width: 560px) {
  .ebook-table .ebook-col-series { display: none; }
  .ebook-table thead th,
  .ebook-table tbody td { padding: 0.4rem 0.5rem; }
  .ebook-thumb { width: 22px; height: 32px; }
}

/* ===== F1.7 Atrium — Library views (header / toolbar / chips / grid) ===== */

/* Editorial header above the library — small mono kicker, large serif title,
   toolbar buttons on the right. */
.lib-header {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 24px 0 18px;
}
.lib-header-kicker {
  display: flex;
  align-items: baseline;
  gap: 8px;
  color: var(--ink-2);
}
.lib-header-path { color: var(--ink-3); }
/* The kicker is the semantic <h1>; visually it stays a small mono label so
   the cinematic count below remains the dominant element. Selector is
   doubled to outrank `.atrium h1` (which would otherwise re-apply 64px). */
.lib-header-kicker-title.lib-header-kicker-title {
  font-family: var(--mono);
  font-size: 10.5px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--ink-2);
  font-weight: 500;
  line-height: 1.45;
  margin: 0;
}
.lib-header-row {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 18px;
  flex-wrap: wrap;
}
.lib-header-title {
  font-family: var(--serif);
  font-size: clamp(40px, 6vw, 64px);
  line-height: 1.0;
  letter-spacing: -0.025em;
  margin: 0;
  color: var(--ink-0);
}
.lib-header-title em {
  font-style: italic;
  font-feature-settings: 'lnum';
}
.lib-header-hint { color: var(--ink-2); font-size: 13px; }

/* Toolbar — Filters / Table / Grid pills plus optional sort cluster. */
.lib-toolbar {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
.lib-view-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.lib-toggle-btn {
  display: inline-flex;
  align-items: center;
  height: 28px;
  padding: 0 10px;
  background: transparent;
  color: var(--ink-1);
  border: 1px solid var(--line-2);
  border-radius: 8px;
  font: inherit;
  font-weight: 500;
  font-size: 12px;
  cursor: pointer;
  transition: background .15s, color .15s, border-color .15s;
}
.lib-toggle-btn:hover { color: var(--ink-0); background: var(--bg-1); border-color: var(--line); }
.lib-toggle-btn[aria-pressed="true"] {
  background: var(--bg-2);
  color: var(--ink-0);
  border-color: var(--line);
}
.lib-filters-btn { /* same look as view toggle */ }

.lib-sort-controls {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-left: 4px;
  padding-left: 10px;
  border-left: 1px solid var(--line-2);
}
.lib-sort-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-family: var(--mono);
  font-size: 10.5px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--ink-2);
  font-weight: 500;
}
.lib-sort-select {
  background: var(--bg-2);
  border: 1px solid var(--line-2);
  border-radius: 8px;
  color: var(--ink-0);
  font: inherit;
  font-size: 12px;
  padding: 4px 8px;
  height: 28px;
}
.lib-sort-select:focus { outline: none; border-color: var(--accent); }
.lib-sort-dir {
  background: var(--bg-2);
  border: 1px solid var(--line-2);
  border-radius: 8px;
  color: var(--ink-0);
  font: inherit;
  padding: 0 10px;
  height: 28px;
  cursor: pointer;
}
.lib-sort-dir:hover { border-color: var(--accent); }

/* Format chip row — inline below the header. */
.lib-format-chips {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding-bottom: 18px;
}
.lib-format-chips-label { margin-right: 4px; }
.lib-format-chips-spacer { flex: 1; }
.lib-format-chips-count {
  color: var(--ink-3);
  font-size: 11.5px;
}

.lib-layout {
  display: grid;
  grid-template-columns: 220px 1fr;
  gap: 24px;
  margin-top: 4px;
  align-items: start;
}
.lib-layout--collapsed {
  grid-template-columns: 1fr;
}
.lib-layout--collapsed > .lib-sidebar { display: none; }

@media (max-width: 900px) {
  .lib-layout { grid-template-columns: 1fr; }
  .lib-layout > .lib-sidebar {
    position: fixed;
    top: 4rem;
    right: 0.75rem;
    z-index: 50;
    width: min(280px, calc(100vw - 1.5rem));
    max-height: calc(100vh - 5rem);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55);
    background: var(--bg-1);
  }
}

.lib-sidebar {
  background: var(--bg-1);
  border: 1px solid var(--line-2);
  border-radius: 14px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  position: sticky;
  top: 80px;
  max-height: calc(100vh - 6rem);
  overflow-y: auto;
}
.lib-clear-filters {
  align-self: flex-start;
  background: transparent;
  border: 1px solid var(--line-2);
  color: var(--ink-1);
  border-radius: 9999px;
  padding: 4px 10px;
  font: inherit;
  font-size: 11.5px;
  cursor: pointer;
  transition: color .15s, border-color .15s, background .15s;
}
.lib-clear-filters:hover { color: var(--ink-0); border-color: var(--accent); background: var(--bg-2); }

.lib-facet { display: flex; flex-direction: column; gap: 8px; }
.lib-facet-title {
  font-family: var(--mono);
  font-size: 10.5px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--ink-2);
  font-weight: 500;
}
.lib-chip-list {
  list-style: none;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 0;
  margin: 0;
}
/* `.lib-chip` defers to Atrium's `.chip` look (composed via class="chip lib-chip");
   only overrides the bits the facet list needs: long-name clipping. */
.lib-chip { max-width: 100%; text-align: left; }
.lib-chip-label {
  display: inline-block;
  max-width: 11rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}
.lib-chip[aria-pressed="true"] {
  color: var(--ink-0);
  border-color: var(--accent);
  background: var(--bg-2);
}
.lib-chip-count { flex-shrink: 0; }

.lib-main { min-width: 0; }

/* Sortable column headers */
.sort-th .sort-th-btn {
  background: transparent;
  border: 0;
  color: inherit;
  font: inherit;
  text-transform: inherit;
  letter-spacing: inherit;
  cursor: pointer;
  padding: 0;
}
.sort-th[aria-sort="ascending"] .sort-th-btn,
.sort-th[aria-sort="descending"] .sort-th-btn { color: var(--accent); }

/* Cover grid — covers float on the warm dark canvas. The Atrium `Cover`
   component handles the cover render + hover lift via `.cover-link`. */
.lib-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 36px 24px;
  margin-top: 4px;
  padding-bottom: 40px;
}
.lib-tile {
  display: block;
  text-decoration: none;
  cursor: pointer;
}
.lib-tile:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 4px;
  border-radius: 2px;
}
.lib-tile-title {
  margin-top: 10px;
  font-size: 13.5px;
  color: var(--ink-0);
  font-weight: 500;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.lib-tile-author {
  margin-top: 2px;
  font-size: 12px;
  color: var(--ink-2);
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (max-width: 1100px) { .ebook-table .ebook-col-updated { display: none; } }
@media (max-width: 1300px) { .ebook-table .ebook-col-added { display: none; } }
"#;
