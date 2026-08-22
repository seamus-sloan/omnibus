//! ISBN normalization: strip separators, validate the check digit, and fold
//! every valid input to a canonical 13-digit ISBN-13 — the form both providers
//! are queried with. Pure and typed, so the caller branches on the failure.
//! Shared because the server needs the verdict before spending a provider round
//! trip, and the check-in scanner/keypad needs it before sending a decode.

#[cfg(test)]
mod tests;

/// Why an ISBN string was rejected. Typed so the check-in UI can show a precise
/// "that barcode isn't a valid ISBN" message instead of a generic error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IsbnError {
    /// After stripping separators, the input wasn't 10 or 13 digits long.
    #[error("ISBN must be 10 or 13 digits, got {0}")]
    InvalidLength(usize),
    /// A non-digit (other than a trailing `X` on an ISBN-10) was present.
    #[error("ISBN contains invalid characters")]
    InvalidChars,
    /// The check digit didn't match the rest of the number.
    #[error("ISBN has an invalid check digit")]
    InvalidCheckDigit,
}

/// Normalize any valid ISBN-10 or ISBN-13 to a canonical 13-digit ISBN-13.
///
/// Hyphens and spaces are ignored. ISBN-10 (whose check digit may be `X`) is
/// validated then converted to ISBN-13; ISBN-13 is validated in place. Returns
/// [`IsbnError`] for a bad length, stray characters, or a wrong check digit.
pub fn normalize_isbn(raw: &str) -> Result<String, IsbnError> {
    let stripped: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();

    // ASCII-only: a valid ISBN is digits (+ a trailing `X`), so reject any
    // non-ASCII up front. This also makes the `.len()` length check below a true
    // character count (bytes == chars for ASCII) and matches the ASCII-digit
    // expectation elsewhere (metadata-override validation).
    if !stripped.is_ascii() {
        return Err(IsbnError::InvalidChars);
    }

    match stripped.len() {
        10 => {
            validate_isbn10(&stripped)?;
            Ok(isbn10_to_isbn13(&stripped))
        }
        13 => {
            validate_isbn13(&stripped)?;
            Ok(stripped)
        }
        other => Err(IsbnError::InvalidLength(other)),
    }
}

/// Validate an ISBN-10: nine digits + a check digit (0–9 or `X` = 10), where
/// the weighted sum (weights 10..1) is divisible by 11.
///
/// `pub` so callers that need a real (check-digit-verified) ISBN-10 —
/// distinct from `MetadataOverrides::validate`'s deliberately format-only
/// override check — can call it directly rather than duplicating the sum.
pub fn validate_isbn10(s: &str) -> Result<(), IsbnError> {
    let mut sum = 0u32;
    for (i, c) in s.chars().enumerate() {
        let value = match c {
            '0'..='9' => c.to_digit(10).unwrap_or(0),
            // `X` (value 10) is only legal as the final check digit.
            'X' | 'x' if i == 9 => 10,
            _ => return Err(IsbnError::InvalidChars),
        };
        // The caller guarantees a 10-char string, so `i` fits u32 and the
        // impossible fallback just zeroes the weight.
        sum += value * (10 - u32::try_from(i).unwrap_or(10));
    }
    if sum.is_multiple_of(11) {
        Ok(())
    } else {
        Err(IsbnError::InvalidCheckDigit)
    }
}

/// Validate an ISBN-13: thirteen digits whose 1/3-weighted sum is divisible by 10.
fn validate_isbn13(s: &str) -> Result<(), IsbnError> {
    let sum = weighted_isbn13_sum(s.chars().take(13))?;
    if sum.is_multiple_of(10) {
        Ok(())
    } else {
        Err(IsbnError::InvalidCheckDigit)
    }
}

/// Convert a *validated* ISBN-10 to ISBN-13: prefix `978`, keep the first nine
/// digits, and recompute the ISBN-13 check digit.
fn isbn10_to_isbn13(isbn10: &str) -> String {
    let mut body = String::with_capacity(13);
    body.push_str("978");
    body.push_str(&isbn10[..9]);
    let check = isbn13_check_digit(&body);
    body.push(char::from_digit(check, 10).unwrap_or('0'));
    body
}

/// The ISBN-13 check digit for a 12-digit body.
fn isbn13_check_digit(body12: &str) -> u32 {
    let sum = weighted_isbn13_sum(body12.chars()).unwrap_or(0);
    (10 - (sum % 10)) % 10
}

/// Sum ISBN-13 digits with alternating weights 1,3,1,3,…; errors on a non-digit.
fn weighted_isbn13_sum(chars: impl Iterator<Item = char>) -> Result<u32, IsbnError> {
    let mut sum = 0u32;
    for (i, c) in chars.enumerate() {
        let digit = c.to_digit(10).ok_or(IsbnError::InvalidChars)?;
        sum += digit * if i.is_multiple_of(2) { 1 } else { 3 };
    }
    Ok(sum)
}
