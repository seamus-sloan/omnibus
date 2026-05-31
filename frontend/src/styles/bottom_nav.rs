//! Styles for the mobile-target bottom navigation bar
//! (`components::bottom_nav`).

pub const STYLES: &str = r#"
.bottom-nav {
  position: fixed;
  bottom: 0; left: 0; right: 0;
  display: flex;
  background: rgba(15, 23, 42, 0.95);
  border-top: 1px solid rgba(100, 116, 139, 0.3);
  padding-bottom: env(safe-area-inset-bottom);
}
.bottom-nav a {
  flex: 1;
  padding: 0.9rem;
  text-align: center;
  color: #94a3b8;
  text-decoration: none;
  font-size: 0.9rem;
}
.bottom-nav a.active { color: #22d3ee; }
"#;
