use dioxus::prelude::*;

/// Bounded password-strength score: 0 (none) through 4 (excellent).
/// Wrap the raw `u8` so out-of-range inputs collapse cleanly to the
/// nearest endpoint rather than over-filling the meter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StrengthScore(u8);

impl StrengthScore {
    pub const MAX: u8 = 4;

    /// Build a clamped score from any `u8`. Values above [`Self::MAX`]
    /// saturate to `MAX`; values below 0 are impossible for `u8`.
    pub fn new(raw: u8) -> Self {
        Self(raw.min(Self::MAX))
    }

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
                aria_valuemin: "0",
                aria_valuemax: "{StrengthScore::MAX}",
                aria_valuenow: "{filled}",
                for i in 0..StrengthScore::MAX {
                    div {
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

#[cfg(test)]
mod tests {
    use super::*;

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
