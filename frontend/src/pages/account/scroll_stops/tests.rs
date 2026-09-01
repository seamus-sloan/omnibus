//! Pure tests for the scroll-stops card's status line. The toggle handler's
//! guards and its failure revert are exercised through the same shape as the
//! self-registration switch it mirrors; only the copy is unique here.

use super::*;

#[test]
fn scroll_stops_status_line_describes_how_the_page_reads() {
    if PARKED {
        // Parked, so the line must not promise a mode the page won't enter,
        // whatever the stored value happens to be.
        for stored in [None, Some(true), Some(false)] {
            assert_eq!(
                scroll_stops_status_line(stored),
                "Book details scroll continuously, top to bottom."
            );
        }
        return;
    }
    assert_eq!(scroll_stops_status_line(None), "Checking…");
    assert_eq!(
        scroll_stops_status_line(Some(true)),
        "Book details snap through one panel at a time."
    );
    assert_eq!(
        scroll_stops_status_line(Some(false)),
        "Book details scroll continuously, top to bottom."
    );
}
