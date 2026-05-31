//! Styles for the Settings page (`pages::settings`) — form fields,
//! status line, and the per-library card with its file list.

/// CSS chunk for the settings page.
pub const STYLES: &str = r#"
.settings-form {
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
  margin-top: 1.25rem;
}
.settings-field { display: flex; flex-direction: column; gap: 0.4rem; }
.settings-field label, .settings-label {
  font-size: 0.875rem;
  font-weight: 500;
  color: #cbd5e1;
}
.settings-field input[type="text"],
.settings-field input[type="password"],
.settings-field input[type="email"],
.settings-input {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font-size: 0.95rem;
  padding: 0.55rem 0.75rem;
  width: 100%;
}
.settings-field input[type="text"]:focus,
.settings-field input[type="password"]:focus,
.settings-field input[type="email"]:focus,
.settings-input:focus {
  outline: none;
  border-color: #3b82f6;
}

.settings-status { font-size: 0.875rem; margin-top: 0.5rem; min-height: 1.2em; }
.settings-status.success, .success-msg { color: #34d399; }
.settings-status.error, .error { color: #f87171; font-size: 0.85rem; }

.library-card { margin-top: 1.25rem; }
.library-card h2, .library-title {
  font-size: 1rem;
  font-weight: 600;
  margin-bottom: 0.75rem;
  color: #cbd5e1;
}
.library-path { font-size: 0.8rem; color: #64748b; font-family: monospace; margin-bottom: 0.4rem; }
.library-count { font-size: 0.85rem; color: #94a3b8; margin-bottom: 0.5rem; }
.library-loading, .library-empty { color: #64748b; font-size: 0.875rem; }

.library-file-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  max-height: 320px;
  overflow-y: auto;
}
.library-file-list li, .library-file {
  font-size: 0.875rem;
  font-family: monospace;
  padding: 0.3rem 0.5rem;
  background: rgba(30, 41, 59, 0.5);
  border-radius: 6px;
  color: #e2e8f0;
}
"#;
