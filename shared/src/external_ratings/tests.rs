//! Tests for [`ProviderRating::new`]'s absent-is-not-zero rule and the
//! attribution [`ExternalRating::new`] adds.

use super::*;

#[test]
fn provider_rating_new_keeps_a_real_score_on_its_own_scale() {
    let r = ProviderRating::new(Some(4.2), 5.0, Some(1_840), None)
        .expect("a real score should survive");

    assert_eq!(r.rating, 4.2);
    assert_eq!(r.rating_max, 5.0);
    assert_eq!(r.ratings_count, Some(1_840));
}

#[test]
fn provider_rating_new_returns_none_when_the_provider_reported_nothing() {
    assert!(ProviderRating::new(None, 5.0, Some(12), None).is_none());
}

#[test]
fn provider_rating_new_treats_a_zero_score_as_absent() {
    // Providers signal "nobody has rated this" with a `0` as readily as with
    // an omitted field; storing it would render as a "0/5" verdict.
    assert!(ProviderRating::new(Some(0.0), 5.0, Some(0), None).is_none());
}

#[test]
fn provider_rating_new_rejects_a_score_above_the_scale() {
    assert!(ProviderRating::new(Some(5.5), 5.0, None, None).is_none());
}

#[test]
fn provider_rating_new_rejects_a_non_finite_score_or_scale() {
    assert!(ProviderRating::new(Some(f64::NAN), 5.0, None, None).is_none());
    assert!(ProviderRating::new(Some(4.0), f64::INFINITY, None, None).is_none());
    assert!(ProviderRating::new(Some(4.0), 0.0, None, None).is_none());
}

#[test]
fn provider_rating_new_drops_a_zero_count_but_keeps_the_score() {
    let r = ProviderRating::new(Some(4.0), 5.0, Some(0), None).expect("the score is still real");

    assert_eq!(r.ratings_count, None);
}

#[test]
fn provider_rating_new_drops_an_oversized_source_url_rather_than_truncating_it() {
    let long = format!(
        "https://x/{}",
        "a".repeat(ProviderRating::SOURCE_URL_MAX_LEN)
    );
    let r = ProviderRating::new(Some(4.0), 5.0, None, Some(long)).expect("the score is still real");

    assert_eq!(r.source_url, None);
}

#[test]
fn external_rating_new_carries_the_source_display_name() {
    let raw = ProviderRating::new(Some(3.5), 5.0, Some(9), None).unwrap();

    let attributed = ExternalRating::new(MetadataProvider::GoogleBooks, raw, 1_700_000_000);

    assert_eq!(attributed.provider, MetadataProvider::GoogleBooks);
    assert_eq!(attributed.display_name, "Google Books");
    assert_eq!(attributed.fetched_at, 1_700_000_000);
}
