//! Styles for the auth pages (LoginPage / RegisterPage) and the F1.6
//! auth-UI primitives (AuthShell, Field, Banner, StrengthMeter). Tokens
//! are `--auth-*` prefixed so they don't collide with the rest of the
//! app.

/// CSS chunk for the login / register / auth shell pages.
pub const STYLES: &str = r#"
.auth-footer { font-size: 0.85rem; color: #94a3b8; margin-top: 1rem; text-align: center; }
.auth-footer a { color: #22d3ee; }

/* ---- F1.6 auth primitives (AuthShell / Field / Banner / StrengthMeter) ----
   Self-contained block. Tokens are `--auth-*` prefixed and declared on
   `:root` so each primitive renders correctly whether or not it's nested
   under an AuthShell. No effect on the rest of the app — nothing outside
   this block references the prefixed tokens. */
:root {
  --auth-ink-0: #f8fafc;
  --auth-ink-1: #cbd5e1;
  --auth-ink-2: #94a3b8;
  --auth-ink-3: #64748b;
  --auth-bg-0: rgba(15, 23, 42, 0.92);
  --auth-bg-1: rgba(15, 23, 42, 0.65);
  --auth-bg-2: rgba(30, 41, 59, 0.5);
  --auth-line: rgba(100, 116, 139, 0.25);
  --auth-line-2: rgba(100, 116, 139, 0.45);
  --auth-accent: #22d3ee;
  --auth-accent-ink: #03131c;
  --auth-ok: #34d399;
  --auth-warn: #fbbf24;
  --auth-bad: #f87171;
  --auth-info: #60a5fa;
  --auth-sans: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  --auth-serif: "Iowan Old Style", "Palatino Linotype", Palatino, Georgia, serif;
  --auth-mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
}

.auth-shell-grid {
  display: grid;
  grid-template-columns: 1.1fr 1fr;
  min-height: 100vh;
  color: var(--auth-ink-0);
  font-family: var(--auth-sans);
}

.auth-shell-art {
  background: var(--auth-bg-1);
  border-right: 1px solid var(--auth-line-2);
  padding: 3rem 3.5rem;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
}

/* Mobile collapse: hides the art panel below 720px. Placed *after* the
   base .auth-shell-art rule so the cascade-by-source-order picks
   `display: none` instead of the base `display: flex`. */
@media (max-width: 720px) {
  .auth-shell-grid { grid-template-columns: 1fr; }
  .auth-shell-art { display: none; }
}
.auth-shell-brand { display: flex; align-items: center; gap: 0.6rem; }
.auth-shell-brand-mark {
  width: 22px;
  height: 22px;
  border-radius: 6px;
  background: linear-gradient(135deg, var(--auth-accent), #3b82f6);
}
.auth-shell-brand-word {
  font-family: var(--auth-serif);
  font-size: 1.25rem;
  letter-spacing: 0.04em;
}

/* Decorative bookshelf: a row of upright books standing on a wooden
   plank. Anchored to the top of the art panel so the tagline below sits
   on its own ground line. Books are pure block elements (no rotation)
   with varied widths/heights to mimic the casual library look. Each
   spine has a small decorative dot rendered via a child span. */
.auth-shell-shelf {
  position: absolute;
  left: 3.5rem; right: 3.5rem; top: 6rem;
  pointer-events: none;
  z-index: 1;
}
.auth-shell-spines {
  display: flex;
  align-items: flex-end;
  justify-content: center;
  gap: 0.55rem;
  min-height: 360px;
}
.auth-shell-spine {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 2px 2px 0 0;
  transform-origin: bottom center;
  /* board edge: subtle spine highlight + shadow + drop shadow on the shelf */
  box-shadow:
    inset 1px 0 0 rgba(255, 255, 255, 0.14),
    inset -1px 0 0 rgba(0, 0, 0, 0.28),
    inset 0 -8px 12px -8px rgba(0, 0, 0, 0.45),
    0 6px 8px -4px rgba(0, 0, 0, 0.55);
}
/* Two horizontal binding bands per spine — the dark strips that wrap
   around the spine of a real bound book. Positioned near top and
   bottom so the centerfield stays clean for the optional dot motif. */
.auth-shell-spine::before,
.auth-shell-spine::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  height: 2px;
  background: rgba(0, 0, 0, 0.32);
  pointer-events: none;
}
.auth-shell-spine::before { top: 10%; }
.auth-shell-spine::after  { bottom: 10%; }
/* Varied widths (28–52px) and heights (250–340px). A few spines tilt
   slightly so the shelf reads "casual library" rather than "soldiers
   on parade." Tilts stay subtle (≤2°) so the binding bands stay
   readable. */
.auth-shell-spine-0 { width: 36px; height: 280px; }
.auth-shell-spine-1 { width: 44px; height: 320px; }
.auth-shell-spine-2 { width: 28px; height: 240px; transform: rotate(-1.8deg); }
.auth-shell-spine-3 { width: 50px; height: 340px; }
.auth-shell-spine-4 { width: 40px; height: 300px; }
.auth-shell-spine-5 { width: 32px; height: 260px; transform: rotate(1.5deg); }
.auth-shell-spine-6 { width: 46px; height: 310px; }
.auth-shell-spine-7 { width: 36px; height: 280px; transform: rotate(-1.1deg); }
.auth-shell-spine-8 { width: 42px; height: 290px; }
/* Small motif circle — only on a curated subset (spines 1, 3, 6) so
   the shelf doesn't read as "every book has the same sticker." */
.auth-shell-spine-dot {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: 1.5px solid rgba(0, 0, 0, 0.4);
  background: transparent;
  opacity: 0.75;
  display: none;
}
.auth-shell-spine-1 .auth-shell-spine-dot,
.auth-shell-spine-3 .auth-shell-spine-dot,
.auth-shell-spine-6 .auth-shell-spine-dot {
  display: block;
}
/* Wooden plank under the books — warm brown gradient with a darker
   shadow line below to suggest a real shelf. */
.auth-shell-shelf-plank {
  height: 14px;
  margin-top: -2px;
  border-radius: 2px;
  background:
    linear-gradient(180deg,
      oklch(0.42 0.04 55) 0%,
      oklch(0.34 0.04 55) 55%,
      oklch(0.26 0.03 55) 100%);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.08),
    0 10px 16px -8px rgba(0, 0, 0, 0.7);
}

.auth-shell-tagline { margin-top: auto; max-width: 460px; position: relative; z-index: 2; }
.auth-shell-headline {
  font-family: var(--auth-serif);
  font-size: clamp(2.4rem, 5vw, 3.5rem);
  line-height: 1;
  margin: 0;
}
.auth-shell-headline-em { font-style: italic; }
.auth-shell-blurb {
  margin-top: 1rem;
  color: var(--auth-ink-1);
  font-size: 0.95rem;
  line-height: 1.55;
}
.auth-shell-meta {
  margin-top: 1.4rem;
  display: flex;
  gap: 1rem;
  font-family: var(--auth-mono);
  font-size: 0.7rem;
  color: var(--auth-ink-3);
  text-transform: uppercase;
  letter-spacing: 0.14em;
}

.auth-shell-form {
  display: grid;
  place-items: center;
  padding: 3rem 2rem;
}
.auth-shell-form-inner { width: 100%; max-width: 420px; }
.auth-shell-kicker {
  font-family: var(--auth-mono);
  font-size: 0.72rem;
  color: var(--auth-ink-3);
  text-transform: uppercase;
  letter-spacing: 0.14em;
}
.auth-shell-title {
  margin: 0.4rem 0 0;
  font-family: var(--auth-serif);
  font-size: 1.9rem;
  line-height: 1.15;
}
.auth-shell-lede {
  margin-top: 0.7rem;
  color: var(--auth-ink-2);
  font-size: 0.9rem;
  line-height: 1.55;
}
.auth-shell-body { margin-top: 1.6rem; }

/* Field */
/* `position: relative` anchors the absolutely-positioned `.auth-field-action`
   to the field box. The action slot lives after the input in DOM order
   (see `components/auth/field.rs`) so tab order is label → input → action;
   absolute positioning pulls it back to the visual top-right next to the
   label. */
.auth-field { display: block; margin-bottom: 0.9rem; position: relative; }
.auth-field-label-row {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  margin-bottom: 0.35rem;
}
.auth-field-label {
  font-family: var(--auth-mono);
  font-size: 0.72rem;
  color: var(--auth-ink-2);
  text-transform: uppercase;
  letter-spacing: 0.1em;
}
.auth-field-action {
  position: absolute;
  top: 0;
  right: 0;
  font-size: 0.78rem;
  color: var(--auth-accent);
  /* Baseline-align with the label row (0.72rem uppercase). */
  line-height: 1;
}
.auth-field-input-wrap { position: relative; }
.auth-field input,
.auth-field textarea,
.auth-field select {
  width: 100%;
  padding: 0.75rem 0.9rem;
  background: var(--auth-bg-1);
  border: 1px solid var(--auth-line-2);
  border-radius: 10px;
  color: var(--auth-ink-0);
  font-size: 0.9rem;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
}
.auth-field input:focus,
.auth-field textarea:focus,
.auth-field select:focus { border-color: var(--auth-accent); }
.auth-field-err input,
.auth-field-err textarea,
.auth-field-err select {
  border-color: var(--auth-bad);
  box-shadow: 0 0 0 3px color-mix(in oklch, var(--auth-bad) 18%, transparent);
}
.auth-field-ok input,
.auth-field-ok textarea,
.auth-field-ok select {
  border-color: var(--auth-ok);
  box-shadow: 0 0 0 3px color-mix(in oklch, var(--auth-ok) 18%, transparent);
}
.auth-field-check {
  position: absolute;
  right: 0.75rem;
  top: 50%;
  transform: translateY(-50%);
  color: var(--auth-ok);
  font-size: 1rem;
  line-height: 1;
}
.auth-field-msg {
  margin-top: 0.4rem;
  font-size: 0.78rem;
  line-height: 1.4;
}
.auth-field-msg-err { color: var(--auth-bad); }
.auth-field-msg-hint {
  color: var(--auth-ink-3);
  font-family: var(--auth-mono);
  font-size: 0.7rem;
  letter-spacing: 0.02em;
}

/* Banner */
.auth-banner {
  display: flex;
  gap: 0.75rem;
  align-items: flex-start;
  padding: 0.85rem 1rem;
  margin-bottom: 1rem;
  border: 1px solid var(--auth-line-2);
  border-radius: 10px;
  background: var(--auth-bg-1);
}
.auth-banner-err   { border-left: 3px solid var(--auth-bad); }
.auth-banner-warn  { border-left: 3px solid var(--auth-warn); }
.auth-banner-info  { border-left: 3px solid var(--auth-info); }
.auth-banner-ok    { border-left: 3px solid var(--auth-ok); }
.auth-banner-icon {
  width: 20px;
  height: 20px;
  display: grid;
  place-items: center;
  border-radius: 999px;
  font-weight: 700;
  font-size: 0.75rem;
  flex: 0 0 20px;
  background: var(--auth-bg-2);
}
.auth-banner-err  .auth-banner-icon { color: var(--auth-bad); }
.auth-banner-warn .auth-banner-icon { color: var(--auth-warn); }
.auth-banner-info .auth-banner-icon { color: var(--auth-info); }
.auth-banner-ok   .auth-banner-icon { color: var(--auth-ok); }
.auth-banner-body { flex: 1; }
.auth-banner-title { font-weight: 500; font-size: 0.85rem; color: var(--auth-ink-0); }
.auth-banner-message { margin-top: 0.25rem; font-size: 0.8rem; color: var(--auth-ink-1); line-height: 1.5; }
.auth-banner-action { margin-top: 0.6rem; display: flex; gap: 0.5rem; }
.auth-banner-dismiss {
  background: transparent;
  border: 0;
  color: var(--auth-ink-3);
  cursor: pointer;
  padding: 0.1rem 0.3rem;
  font: inherit;
  align-self: flex-start;
}
.auth-banner-dismiss:hover { color: var(--auth-ink-0); }

/* StrengthMeter */
.auth-strength { margin-top: 0.5rem; }
.auth-strength-bar {
  display: flex;
  gap: 0.25rem;
}
.auth-strength-segment {
  flex: 1;
  height: 3px;
  border-radius: 999px;
  background: var(--auth-bg-2);
}
.auth-strength-tier-bad  .auth-strength-segment-on { background: var(--auth-bad); }
.auth-strength-tier-warn .auth-strength-segment-on { background: var(--auth-warn); }
.auth-strength-tier-mid  .auth-strength-segment-on { background: #eab308; }
.auth-strength-tier-ok   .auth-strength-segment-on { background: var(--auth-ok); }
.auth-strength-label {
  margin-top: 0.4rem;
  display: flex;
  justify-content: space-between;
  font-family: var(--auth-mono);
  font-size: 0.68rem;
  color: var(--auth-ink-3);
  letter-spacing: 0.04em;
}
.auth-strength-label-rhs.auth-strength-tier-bad  { color: var(--auth-bad); }
.auth-strength-label-rhs.auth-strength-tier-warn { color: var(--auth-warn); }
.auth-strength-label-rhs.auth-strength-tier-mid  { color: #eab308; }
.auth-strength-label-rhs.auth-strength-tier-ok   { color: var(--auth-ok); }

/* Page-level adornments consumed by LoginPage / RegisterPage on top of the
   F1.6 primitives. Stays in the auth-* namespace so a future move to a
   dedicated stylesheet is grep-friendly. */
.auth-form-inner { display: block; }
.auth-submit {
  width: 100%;
  justify-content: center;
  margin-top: 0.4rem;
}
.auth-field-action-link {
  color: var(--auth-accent);
  font-size: 0.78rem;
  text-decoration: none;
}
.auth-field-action-link:hover { text-decoration: underline; }

.auth-checkbox {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  margin: 0.4rem 0 0.8rem;
  font-size: 0.85rem;
  color: var(--auth-ink-1);
  cursor: pointer;
}
.auth-checkbox-block { align-items: flex-start; line-height: 1.5; }
.auth-checkbox input[type="checkbox"] {
  width: 16px;
  height: 16px;
  flex: 0 0 16px;
  accent-color: var(--auth-accent);
  margin-top: 1px;
}

.auth-requirements {
  margin-top: 0.9rem;
  padding: 0.7rem 0.85rem;
  background: var(--auth-bg-2);
  border: 1px solid var(--auth-line-2);
  border-radius: 10px;
}
.auth-requirements-title {
  font-family: var(--auth-mono);
  font-size: 0.7rem;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--auth-ink-2);
  margin-bottom: 0.5rem;
}
.auth-req {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.8rem;
  color: var(--auth-ink-2);
  padding: 0.15rem 0;
}
.auth-req-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--auth-ink-3);
  flex: 0 0 6px;
}
.auth-req.ok { color: var(--auth-ink-0); }
.auth-req.ok .auth-req-dot { background: var(--auth-ok); }

.auth-footer-note {
  margin-top: 1.1rem;
  text-align: center;
  font-family: var(--auth-mono);
  font-size: 0.65rem;
  letter-spacing: 0.12em;
  color: var(--auth-ink-3);
}
"#;
