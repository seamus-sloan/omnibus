//! Four-segment password-strength meter.
//!
//! Renders a clamped [`StrengthScore`] (0–4) as colored bar segments with
//! an accessible label. Used by the register page next to the password
//! input.

use dioxus::prelude::*;

/// Bounded password-strength score: 0 (none) through 4 (excellent).
/// Wrap the raw `u8` so out-of-range inputs collapse cleanly to the
/// nearest endpoint rather than over-filling the meter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StrengthScore(u8);

impl StrengthScore {
    /// Highest score the meter can show; [`Self::new`] saturates to it.
    pub const MAX: u8 = 4;

    /// Build a clamped score from any `u8`. Values above [`Self::MAX`]
    /// saturate to `MAX`; values below 0 are impossible for `u8`.
    pub fn new(raw: u8) -> Self {
        Self(raw.min(Self::MAX))
    }

    /// Return the underlying clamped score (0..=[`Self::MAX`]).
    pub fn value(self) -> u8 {
        self.0
    }

    /// Modifier class for color tiering.
    pub fn tier_class(self) -> &'static str {
        match self.0 {
            0 => "auth-strength-tier-none",
            1 => "auth-strength-tier-bad",
            2 => "auth-strength-tier-warn",
            3 => "auth-strength-tier-mid",
            _ => "auth-strength-tier-ok",
        }
    }
}

impl From<u8> for StrengthScore {
    fn from(raw: u8) -> Self {
        Self::new(raw)
    }
}

/// Four-segment presentational strength bar. **Purely visual** — actual
/// password policy lives on the server. Pass a `label` (e.g. "Strong",
/// "Weak", "Excellent") to render under the bar; an empty label hides
/// the label row.
#[component]
pub fn StrengthMeter(score: StrengthScore, #[props(default)] label: Option<String>) -> Element {
    let filled = score.value();
    let tier = score.tier_class();

    rsx! {
        div { class: "auth-strength",
            div {
                class: "auth-strength-bar {tier}",
                role: "meter",
                aria_label: "Password strength",
                aria_valuemin: "0",
                aria_valuemax: "{StrengthScore::MAX}",
                aria_valuenow: "{filled}",
                for i in 0..StrengthScore::MAX {
                    div {
                        key: "{i}",
                        class: if i < filled { "auth-strength-segment auth-strength-segment-on" } else { "auth-strength-segment" },
                    }
                }
            }
            if let Some(text) = label {
                div { class: "auth-strength-label",
                    span { class: "auth-strength-label-lhs", "strength" }
                    span { class: "auth-strength-label-rhs {tier}", "{text}" }
                }
            }
        }
    }
}

/// Presentational password scoring — the server still enforces policy (the
/// 10-char minimum + common-password reject-list). Returns the meter score
/// (0..=4), a short label, and the three checklist booleans (length≥10,
/// mixed case, number-or-symbol) so a page renders both the meter and the
/// requirements list from one pass. Shared by the register page and the
/// Settings → Account change-password form.
pub fn score_password(pw: &str) -> (StrengthScore, &'static str, [bool; 3]) {
    let len = pw.chars().count();
    let has_lower = pw.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = pw.chars().any(|c| c.is_ascii_uppercase());
    let mixed_case = has_lower && has_upper;
    let has_number_or_symbol = pw
        .chars()
        .any(|c| c.is_ascii_digit() || !c.is_alphanumeric());
    let len_ok = len >= 10;

    let mut raw: u8 = 0;
    if len >= 4 {
        raw = raw.saturating_add(1);
    }
    if len >= 8 {
        raw = raw.saturating_add(1);
    }
    if mixed_case {
        raw = raw.saturating_add(1);
    }
    if has_number_or_symbol {
        raw = raw.saturating_add(1);
    }
    if len_ok {
        raw = raw.saturating_add(1);
    }
    let score = StrengthScore::new(raw.min(StrengthScore::MAX));
    // "empty" only when nothing was typed; a non-empty score-0 input
    // (1–3 chars, no special chars) is still weak, not empty.
    let label = if pw.is_empty() {
        "empty"
    } else {
        match score.value() {
            0 | 1 => "weak",
            2 => "fair",
            3 => "good",
            _ => "strong",
        }
    };
    (score, label, [len_ok, mixed_case, has_number_or_symbol])
}

/// The three-rule password requirements checklist rendered under the strength
/// meter: length, mixed case, and one number-or-symbol. Pass the `rules`
/// triple from [`score_password`]; each row shows met/unmet with an
/// SR-only status so the dot color isn't the only signal.
#[component]
pub fn PasswordRequirements(rules: [bool; 3]) -> Element {
    rsx! {
        div { class: "auth-requirements",
            div { class: "auth-requirements-title", "Password needs" }
            PasswordRequirementRow { ok: rules[0], text: "At least 10 characters" }
            PasswordRequirementRow { ok: rules[1], text: "Mixed case" }
            PasswordRequirementRow { ok: rules[2], text: "One number or symbol" }
        }
    }
}

#[component]
fn PasswordRequirementRow(ok: bool, text: String) -> Element {
    let cls = if ok { "auth-req ok" } else { "auth-req" };
    let status = if ok { "met" } else { "not met" };
    rsx! {
        div { class: "{cls}",
            span { class: "auth-req-dot", aria_hidden: "true" }
            span { "{text}" }
            // Screen-reader-only status — the dot color alone isn't
            // perceivable to assistive tech, so announce met/unmet.
            span { class: "sr-only", ": {status}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_password_empty_is_zero() {
        let (score, label, rules) = score_password("");
        assert_eq!(score.value(), 0);
        assert_eq!(label, "empty");
        assert_eq!(rules, [false, false, false]);
    }

    #[test]
    fn score_password_grows_with_length_and_classes() {
        let (s, _, _) = score_password("abcd");
        assert_eq!(s.value(), 1);
        let (s, _, _) = score_password("AbCdEfGh");
        assert_eq!(s.value(), 3);
        let (s, label, rules) = score_password("AbCdEfGh1!2x");
        assert_eq!(s.value(), 4);
        assert_eq!(label, "strong");
        assert_eq!(rules, [true, true, true]);
    }

    #[test]
    fn score_password_rules_track_thresholds() {
        let (_, _, rules) = score_password("Ab1");
        assert_eq!(rules, [false, true, true]);
        let (_, _, rules) = score_password("abcdefghijk1");
        assert_eq!(rules, [true, false, true]);
    }

    #[test]
    fn score_password_length_rule_boundary() {
        // Right at the server-policy boundary (10 chars). 9-char inputs
        // must report length-not-met; 10-char inputs must report met.
        let (_, _, rules) = score_password("abcdefgh1");
        assert!(!rules[0], "9-char input must not satisfy len_ok");
        let (_, _, rules) = score_password("abcdefghi1");
        assert!(rules[0], "10-char input must satisfy len_ok");
        let (_, _, rules) = score_password("abcdefghij1");
        assert!(rules[0], "11-char input must satisfy len_ok");
    }

    #[test]
    fn score_password_label_distinguishes_empty_from_short() {
        // Empty -> "empty"; any non-empty input -> at least "weak"
        // (covers the 1–3 char range where score=0 but typed content
        // exists, so the meter shouldn't lie about being empty).
        let (_, label, _) = score_password("");
        assert_eq!(label, "empty");
        let (_, label, _) = score_password("a");
        assert_eq!(label, "weak");
        let (_, label, _) = score_password("ab");
        assert_eq!(label, "weak");
    }

    #[test]
    fn clamps_above_max() {
        assert_eq!(StrengthScore::new(7).value(), StrengthScore::MAX);
    }

    #[test]
    fn passes_through_in_range() {
        for raw in 0..=StrengthScore::MAX {
            assert_eq!(StrengthScore::new(raw).value(), raw);
        }
    }

    #[test]
    fn tier_class_covers_each_score() {
        assert_eq!(
            StrengthScore::new(0).tier_class(),
            "auth-strength-tier-none"
        );
        assert_eq!(StrengthScore::new(1).tier_class(), "auth-strength-tier-bad");
        assert_eq!(
            StrengthScore::new(2).tier_class(),
            "auth-strength-tier-warn"
        );
        assert_eq!(StrengthScore::new(3).tier_class(), "auth-strength-tier-mid");
        assert_eq!(StrengthScore::new(4).tier_class(), "auth-strength-tier-ok");
    }

    #[test]
    fn tier_class_saturates_on_overflow_input() {
        assert_eq!(StrengthScore::new(99).tier_class(), "auth-strength-tier-ok");
    }

    #[test]
    fn from_u8_matches_new() {
        let via_from: StrengthScore = 3u8.into();
        assert_eq!(via_from, StrengthScore::new(3));
    }
}
