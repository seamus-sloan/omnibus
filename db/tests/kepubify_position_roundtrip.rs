//! Round-trips reading positions through a **real** kepubify conversion —
//! the one span-numbering authority the unit fixtures can only imitate.
//! Skips (passes trivially) when the binary is absent, so the slim-shell
//! matrix stays green; run inside `nix develop .#web` to exercise it.

use omnibus_db::kobo_position::{cfi_to_span, parse_location, span_to_cfi, KoboLoc};
use omnibus_db::test_support::{build_test_epub, make_test_dir};

const CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>One</title></head>
<body>
  <h1>Chapter One</h1>
  <p>The first paragraph has two sentences. Here is the second one.</p>
  <p>A follow-up paragraph with <em>inline emphasis</em> mid-sentence. And a closer.</p>
</body>
</html>"#;

/// Run kepubify over `src`, producing `out`. `None` when the binary is
/// unavailable (the test then passes without asserting). Resolves the
/// binary the same way `kepubify_available` does — `$OMNIBUS_KEPUBIFY_PATH`
/// first — so the probe and the spawn can't disagree.
fn kepubify(src: &std::path::Path, out: &std::path::Path) -> Option<()> {
    if !omnibus_db::kepubify_available() {
        eprintln!("kepubify unavailable; skipping real-conversion round-trip");
        return None;
    }
    let bin = std::env::var("OMNIBUS_KEPUBIFY_PATH").unwrap_or_else(|_| "kepubify".to_string());
    let status = std::process::Command::new(&bin)
        .arg("-o")
        .arg(out)
        .arg("--")
        .arg(src)
        .status()
        .expect("spawn kepubify");
    assert!(status.success(), "kepubify failed");
    Some(())
}

#[test]
fn positions_round_trip_through_a_real_kepubify_conversion() {
    let dir = make_test_dir("kepubify_roundtrip");
    let source = dir.join("book.epub");
    std::fs::write(&source, build_test_epub(&[("c1.xhtml", CHAPTER)])).unwrap();
    let kepub = dir.join("book.kepub.epub");
    if kepubify(&source, &kepub).is_none() {
        return;
    }

    // Walk kepubify's actual segmentation: every span the converter minted
    // must map to a CFI and back to the same span.
    let mut round_tripped = 0u32;
    for n in 1..=8u32 {
        for m in 1..=8u32 {
            let loc = KoboLoc {
                source: "c1.xhtml".into(),
                n,
                m,
            };
            let Some(cfi) = span_to_cfi(&kepub, &source, &loc).expect("derive cfi") else {
                continue; // span id not minted by kepubify — fine
            };
            let derived = cfi_to_span(Some(&kepub), &source, &cfi).expect("derive span");
            let back = derived
                .location_json
                .as_deref()
                .and_then(parse_location)
                .unwrap_or_else(|| panic!("kobo.{n}.{m} → {cfi} → no span"));
            assert_eq!(
                (back.n, back.m),
                (n, m),
                "kobo.{n}.{m} must round-trip through {cfi}"
            );
            assert!(derived.percent.is_some());
            round_tripped += 1;
        }
    }
    assert!(
        round_tripped >= 3,
        "kepubify should have minted several spans; only {round_tripped} round-tripped"
    );
}
