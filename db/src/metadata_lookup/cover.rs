//! The terms a provider cover is fetched under.
//!
//! Applying a source's cover means the server fetching a client-supplied URL,
//! which is a textbook SSRF gadget aimed at the server's own network. The
//! allowlist is the control, not a nicety — and it is [`super::cover_hosts`],
//! the same list the `img-src` CSP is built from, so a host the picker can
//! render is exactly a host a cover can be applied from.

use crate::author_photos::{fetch_remote_image_with, FetchRemoteImageError, RemoteImageConfig};

use super::providers::googlebooks;

/// Redirect hops allowed when fetching a provider cover.
///
/// Not zero, because Open Library's cover CDN genuinely needs two: it 302s to
/// `archive.org`, which 302s again to the node holding the file. Four leaves
/// headroom without turning the fetch into a crawl. Every hop is re-checked
/// against the same scheme and allowlist gates as the original URL — the
/// follow is only safe because of that.
pub const MAX_COVER_REDIRECTS: u8 = 4;

/// The fetch config for a provider cover: HTTPS only, hosts limited to the
/// provider catalog's, and a bounded redirect follow.
///
/// `allow_private_addresses` is threaded through rather than defaulted so a
/// test can point the fetch at a loopback `wiremock` origin; production
/// callers pass `false` and every other gate still applies either way.
pub fn provider_cover_image_config(allow_private_addresses: bool) -> RemoteImageConfig {
    RemoteImageConfig {
        allow_private_addresses,
        host_allowlist: super::all_cover_hosts()
            .into_iter()
            .map(str::to_string)
            .collect(),
        require_https: true,
        max_redirects: MAX_COVER_REDIRECTS,
    }
}

/// How far a high-resolution candidate's aspect ratio may drift from the
/// thumbnail's before it is judged a different picture.
///
/// Measured across 18 volumes: every genuine high-resolution cover matched its
/// own thumbnail within 2.6%, and every placeholder missed by 5.4% or more.
/// The threshold sits in that gap rather than hugging either edge.
const COVER_ASPECT_TOLERANCE: f64 = 0.04;

/// Fetch a provider cover, taking the highest resolution that is provably the
/// same picture.
///
/// For every provider but one this is [`fetch_remote_image_with`] unchanged.
/// Google Books is the exception: its `imageLinks` only publishes 128px
/// renditions, so the URL the picker holds is always the small one, and the
/// full-size art has to be asked for separately.
///
/// That request cannot be trusted on its status. A volume Google holds no art
/// for answers **200 with a placeholder** — an "image not available" card, a
/// redacted-cover skeleton, or the publisher's colophon in place of the real
/// jacket — so the bytes are compared against the thumbnail before being
/// accepted, and the thumbnail is kept whenever they disagree. Roughly two
/// thirds of volumes take that path, which is why the upgrade is opportunistic
/// rather than a plain URL rewrite at search time.
///
/// Both fetches run under the caller's `config`, so the host allowlist,
/// HTTPS-only rule, redirect cap and size cap apply to the upgrade exactly as
/// they do to the original.
pub async fn fetch_provider_cover(
    url: &str,
    config: &RemoteImageConfig,
) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    let Some(upgraded_url) = googlebooks::upgrade_cover_url(url) else {
        return fetch_remote_image_with(url, config).await;
    };

    let (thumbnail, upgraded) = tokio::join!(
        fetch_remote_image_with(url, config),
        fetch_remote_image_with(&upgraded_url, config),
    );
    let thumbnail = thumbnail?;

    // Best-effort by construction: the thumbnail is a real cover, so anything
    // that goes wrong reaching for a bigger one is a reason to keep what we
    // already have rather than to fail the reader's apply.
    let Ok(upgraded) = upgraded else {
        return Ok(thumbnail);
    };
    if is_larger_rendition(&thumbnail.1, &upgraded.1) {
        Ok(upgraded)
    } else {
        Ok(thumbnail)
    }
}

/// Whether `candidate` is a bigger rendition of `thumbnail` rather than a
/// different image.
///
/// Aspect ratio is the discriminator because the thumbnail is always the real
/// cover, and a rendition of a picture keeps its shape while a substituted
/// placeholder does not. Undecodable bytes on either side answer `false`: the
/// thumbnail is the safe choice, and the caller's own magic-byte sniff is what
/// rejects a non-image outright.
fn is_larger_rendition(thumbnail: &[u8], candidate: &[u8]) -> bool {
    let (Some((tw, th)), Some((cw, ch))) = (dimensions(thumbnail), dimensions(candidate)) else {
        return false;
    };
    // No point paying for a second image that is not actually bigger.
    if cw <= tw {
        return false;
    }
    let (Some(thumb_ratio), Some(cand_ratio)) = (ratio(tw, th), ratio(cw, ch)) else {
        return false;
    };
    (thumb_ratio - cand_ratio).abs() / thumb_ratio <= COVER_ASPECT_TOLERANCE
}

/// Width-over-height, or `None` for a zero dimension a decoder should never
/// have produced.
fn ratio(width: u32, height: u32) -> Option<f64> {
    (width > 0 && height > 0).then(|| f64::from(width) / f64::from(height))
}

/// Pixel dimensions from an image header, without decoding the pixels.
fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use omnibus_shared::metadata_lookup::MetadataProvider;

    use super::*;
    use crate::metadata_lookup::cover_hosts;

    #[test]
    fn provider_cover_config_allows_every_host_the_catalog_publishes() {
        let config = provider_cover_image_config(false);
        for provider in MetadataProvider::ALL.iter().copied() {
            for host in cover_hosts(provider) {
                assert!(
                    config.host_allowlist.iter().any(|h| h == host),
                    "{host} renders in the picker but could not be applied"
                );
            }
        }
    }

    #[test]
    fn provider_cover_config_is_https_only_and_follows_a_bounded_number_of_hops() {
        let config = provider_cover_image_config(false);
        assert!(config.require_https);
        assert!(!config.allow_private_addresses);
        // Two is the real requirement (Open Library's double redirect); the
        // assertion is that it is bounded, not that it is generous.
        assert!(config.max_redirects >= 2);
        assert!(config.max_redirects <= 8);
    }

    /// A blank JPEG of the given size — `is_larger_rendition` reads the header
    /// only, so the pixels are irrelevant and the shape is the whole fixture.
    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .expect("encode fixture");
        out.into_inner()
    }

    // The dimensions below are measured from Google Books, not invented: the
    // thumbnail is what `imageLinks` publishes, and each candidate is what the
    // upgraded URL actually returned for some volume.

    #[test]
    fn is_larger_rendition_accepts_the_full_size_art_for_the_same_cover() {
        // Dune: 128x192 thumbnail, 1749x2694 original. A 2.6% ratio drift,
        // which is JPEG-era rounding rather than a different picture.
        assert!(is_larger_rendition(&jpeg(128, 192), &jpeg(1749, 2694)));
    }

    #[test]
    fn is_larger_rendition_rejects_the_image_not_available_placeholder() {
        // The commonest substitution: 11 of 18 sampled volumes answered the
        // upgraded URL with this one card, byte-identical, at a 200.
        assert!(!is_larger_rendition(&jpeg(128, 192), &jpeg(575, 750)));
    }

    #[test]
    fn is_larger_rendition_rejects_the_redacted_cover_skeleton() {
        // A second placeholder, and the reason file size cannot be the signal:
        // this one is a 246 KB drawing, far heavier than a real small cover.
        assert!(!is_larger_rendition(&jpeg(128, 198), &jpeg(575, 829)));
    }

    #[test]
    fn is_larger_rendition_rejects_a_candidate_that_is_no_bigger() {
        // Nothing gained, and a second fetch already spent.
        assert!(!is_larger_rendition(&jpeg(128, 192), &jpeg(128, 192)));
        assert!(!is_larger_rendition(&jpeg(800, 1200), &jpeg(575, 863)));
    }

    #[test]
    fn is_larger_rendition_rejects_bytes_it_cannot_decode() {
        // "Can't tell" resolves to keeping the thumbnail, which is known good.
        assert!(!is_larger_rendition(&jpeg(128, 192), b"not an image"));
        assert!(!is_larger_rendition(b"not an image", &jpeg(800, 1200)));
    }

    #[test]
    fn ratio_refuses_a_zero_dimension() {
        assert_eq!(ratio(0, 100), None);
        assert_eq!(ratio(100, 0), None);
    }
}
