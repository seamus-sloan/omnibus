//! Codec classification for audiobook playback.
//!
//! Decides whether each book can be streamed directly to the browser
//! / native player (m4b / m4a / mp3 / aac) or has to go through the
//! HLS transcode pipeline (flac / ac3 / eac3 / anything else). Used
//! by the `/api/audiobooks/{uuid}/manifest` handler to pick the
//! response variant. The HLS code itself stays unchanged — this is
//! the gate that decides whether to invoke it at all.

/// Playback path for an audiobook. Returned by [`classify_filenames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMode {
    /// Each part can be served as-is over HTTP Range. Frontend wires
    /// a JS playlist controller that advances part-to-part on `ended`.
    Direct,
    /// Falls back to the segmented transcode pipeline in
    /// [`crate::hls`]. The frontend keeps polling `/status` and using
    /// hls.js for these.
    Hls,
}

/// Extensions modern browsers + native audio elements decode without
/// any transcoding. Conservative: only formats with broad cross-
/// browser and cross-platform native support — anything iffy (flac,
/// ogg, opus-in-mp4) gets routed through HLS so the user-visible
/// behaviour is uniform.
const DIRECT_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp3", "aac"];

/// `true` when the file can be served as-is to a standard audio
/// player. Filename is matched on its extension only — we never read
/// the file to detect codec mismatches inside the container; a
/// mislabelled `.mp3` that's actually flac inside will surface as a
/// browser playback error and the user can reindex.
pub fn is_direct_playable(filename: &str) -> bool {
    extension_of(filename)
        .map(|ext| DIRECT_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or(false)
}

/// Decide the playback mode for a multi-part audiobook. A book
/// direct-plays only when EVERY part is independently direct-
/// playable — a single flac mixed into an otherwise mp3 folder
/// routes the whole book through HLS so the cross-part timeline
/// math (cumulative offsets, `ended`-driven advance) does not have
/// to deal with mid-book codec switches.
///
/// Empty input returns `Direct` vacuously; callers are expected to
/// 404 before reaching this point if the book has no parts.
pub fn classify_filenames<S: AsRef<str>>(filenames: &[S]) -> PlaybackMode {
    if filenames.iter().all(|f| is_direct_playable(f.as_ref())) {
        PlaybackMode::Direct
    } else {
        PlaybackMode::Hls
    }
}

/// IANA MIME type for an audiobook file extension. Returns
/// `application/octet-stream` for anything not recognized so the HTTP
/// response is still self-describing rather than missing a header.
pub fn mime_for_filename(filename: &str) -> &'static str {
    match extension_of(filename).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("m4b" | "m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("ogg" | "oga") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Lowercased file extension, no leading dot. `None` when the
/// filename has no recognizable extension.
fn extension_of(filename: &str) -> Option<String> {
    std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_direct_playable_accepts_m4b_m4a_mp3_aac_any_case() {
        assert!(is_direct_playable("foo.mp3"));
        assert!(is_direct_playable("FOO.MP3"));
        assert!(is_direct_playable("path/to/foo.m4b"));
        assert!(is_direct_playable("foo.m4a"));
        assert!(is_direct_playable("foo.aac"));
    }

    #[test]
    fn is_direct_playable_rejects_flac_ac3_extensionless() {
        assert!(!is_direct_playable("foo.flac"));
        assert!(!is_direct_playable("foo.ac3"));
        assert!(!is_direct_playable("foo.ogg"));
        assert!(!is_direct_playable("noextension"));
    }

    #[test]
    fn classify_filenames_returns_direct_when_all_supported() {
        let parts = ["a.mp3", "b.mp3", "c.mp3"];
        assert_eq!(classify_filenames(&parts), PlaybackMode::Direct);
    }

    #[test]
    fn classify_filenames_returns_hls_when_any_unsupported() {
        // The "mixed folder" rule: one flac in the bunch forces the
        // whole book through HLS so cross-part timeline math stays
        // codec-uniform.
        let parts = ["a.mp3", "b.flac", "c.mp3"];
        assert_eq!(classify_filenames(&parts), PlaybackMode::Hls);
    }

    #[test]
    fn classify_filenames_handles_single_m4b() {
        assert_eq!(
            classify_filenames(&["Author/Book.m4b"]),
            PlaybackMode::Direct,
        );
    }

    #[test]
    fn mime_for_filename_maps_known_extensions() {
        assert_eq!(mime_for_filename("a.mp3"), "audio/mpeg");
        assert_eq!(mime_for_filename("a.m4b"), "audio/mp4");
        assert_eq!(mime_for_filename("a.m4a"), "audio/mp4");
        assert_eq!(mime_for_filename("a.aac"), "audio/aac");
        assert_eq!(mime_for_filename("a.flac"), "audio/flac");
        assert_eq!(mime_for_filename("a.OGG"), "audio/ogg");
    }

    #[test]
    fn mime_for_filename_returns_octet_stream_for_unknown() {
        assert_eq!(mime_for_filename("a.xyz"), "application/octet-stream");
        assert_eq!(mime_for_filename("noext"), "application/octet-stream");
    }
}
