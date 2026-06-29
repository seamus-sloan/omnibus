//! Validation and half-star conversion tests for the rating wire types.

use super::*;

#[test]
fn rating_update_accepts_every_half_step() {
    // 0.5, 1.0, 1.5, … 5.0 are all valid.
    for half in 1..=10 {
        let u = RatingUpdate {
            book_uuid: "book-1".into(),
            stars: half as f32 / 2.0,
        };
        assert!(u.validate().is_ok(), "stars={} should be valid", u.stars);
    }
}

#[test]
fn rating_update_half_stars_maps_to_storage_units() {
    let cases = [(0.5, 1), (1.0, 2), (2.5, 5), (4.5, 9), (5.0, 10)];
    for (stars, expected) in cases {
        let u = RatingUpdate {
            book_uuid: "book-1".into(),
            stars,
        };
        assert_eq!(u.half_stars(), expected, "stars={stars}");
    }
}

#[test]
fn stars_from_half_stars_round_trips() {
    for half in 1..=10 {
        assert_eq!(stars_from_half_stars(half), half as f32 / 2.0);
    }
}

#[test]
fn rating_update_rejects_empty_uuid() {
    let u = RatingUpdate {
        book_uuid: "   ".into(),
        stars: 3.0,
    };
    let err = u.validate().expect_err("blank uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn rating_update_rejects_zero() {
    let u = RatingUpdate {
        book_uuid: "book-1".into(),
        stars: 0.0,
    };
    let err = u.validate().expect_err("0 stars must be rejected");
    assert!(err.contains("between"), "got: {err}");
}

#[test]
fn rating_update_rejects_above_max() {
    let u = RatingUpdate {
        book_uuid: "book-1".into(),
        stars: 5.5,
    };
    let err = u.validate().expect_err("5.5 stars must be rejected");
    assert!(err.contains("between"), "got: {err}");
}

#[test]
fn rating_update_rejects_non_half_step() {
    let u = RatingUpdate {
        book_uuid: "book-1".into(),
        stars: 4.3,
    };
    let err = u.validate().expect_err("4.3 is not a half-step");
    assert!(err.contains("half-star"), "got: {err}");
}

#[test]
fn rating_update_rejects_nan() {
    let u = RatingUpdate {
        book_uuid: "book-1".into(),
        stars: f32::NAN,
    };
    assert!(u.validate().is_err(), "NaN must be rejected");
}
