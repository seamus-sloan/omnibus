use super::*;

const ISBN13: &str = "9780134685991";

#[test]
fn normalize_passes_through_valid_isbn13() {
    assert_eq!(normalize_isbn(ISBN13).unwrap(), ISBN13);
}

#[test]
fn normalize_strips_hyphens_and_spaces() {
    assert_eq!(normalize_isbn("978-0-13-468599-1").unwrap(), ISBN13);
    assert_eq!(normalize_isbn("978 0 13 468599 1").unwrap(), ISBN13);
}

#[test]
fn normalize_converts_isbn10_to_isbn13() {
    assert_eq!(normalize_isbn("0134685997").unwrap(), ISBN13);
    assert_eq!(normalize_isbn("0-13-468599-7").unwrap(), ISBN13);
}

#[test]
fn normalize_accepts_isbn10_with_x_check_digit() {
    // 123456789X is a valid ISBN-10 (check digit X = 10) → ISBN-13 9781234567897.
    assert_eq!(normalize_isbn("123456789X").unwrap(), "9781234567897");
    assert_eq!(normalize_isbn("123456789x").unwrap(), "9781234567897");
}

#[test]
fn normalize_rejects_bad_isbn13_check_digit() {
    assert_eq!(
        normalize_isbn("9780134685990"),
        Err(IsbnError::InvalidCheckDigit)
    );
}

#[test]
fn normalize_rejects_bad_isbn10_check_digit() {
    assert_eq!(
        normalize_isbn("0134685996"),
        Err(IsbnError::InvalidCheckDigit)
    );
}

#[test]
fn normalize_rejects_wrong_length() {
    assert_eq!(normalize_isbn("12345"), Err(IsbnError::InvalidLength(5)));
    assert_eq!(normalize_isbn(""), Err(IsbnError::InvalidLength(0)));
}

#[test]
fn normalize_rejects_non_digit_characters() {
    // 13 chars but with letters where digits belong.
    assert_eq!(
        normalize_isbn("97801346859AB"),
        Err(IsbnError::InvalidChars)
    );
    // `X` is only legal as an ISBN-10 trailing check digit, not mid-number.
    assert_eq!(normalize_isbn("01X3468599"), Err(IsbnError::InvalidChars));
}

#[test]
fn normalize_rejects_non_ascii_digit_lookalikes() {
    // Fullwidth digits (U+FF10..) are Unicode "digits" but not ASCII — a valid
    // ISBN never contains them, so they're rejected rather than folded.
    assert_eq!(
        normalize_isbn("９７８０１３４６８５９９１"),
        Err(IsbnError::InvalidChars)
    );
}
