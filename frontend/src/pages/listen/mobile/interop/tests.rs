use super::*;

#[test]
fn surface_js_interpolates_and_has_no_leaked_escapes() {
    let meta = NowPlaying {
        title: "A Sea of Glass",
        author: "Jane Doe",
        artwork_url: Some("http://host/api/thumbs/x/lg?token=t"),
    }
    .to_json_lit();
    let js = surface_js("[{\"url\":\"u\",\"duration\":1}]", "12.5", "1.2", &meta);
    assert!(js.contains("var resume = 12.5;"));
    assert!(js.contains("el.playbackRate = rate;"));
    // WKWebView wipes playbackRate on every load, so the tracked rate is
    // re-applied on loadedmetadata and setRate persists it.
    assert!(js.contains("_rate: rate,"), "tracked rate field missing");
    assert!(
        js.contains("el.playbackRate = oa._rate;"),
        "loadedmetadata rate re-apply missing"
    );
    assert!(
        js.contains("setRate: function(r){ this._rate = r;"),
        "setRate should persist the tracked rate"
    );
    assert!(js.contains("window.OmnibusMobileAudio"));
    assert!(js.contains("dioxus.send"));
    // The timeupdate tick must carry the element's paused state for the drain to reconcile.
    assert!(
        js.contains("kind: 'Time', seconds: absTime(), paused: el.paused"),
        "timeupdate must report the element's paused state"
    );
    // Media Session wiring landed.
    assert!(js.contains("new MediaMetadata"), "media session missing");
    assert!(js.contains("setActionHandler"), "action handlers missing");
    assert!(js.contains("setPositionState"), "position state missing");
    assert!(
        js.contains("playbackState = 'playing'"),
        "playback state missing"
    );
    // No stray `format!` escape pairs leaked into the emitted JS.
    assert!(!js.contains("{{"), "literal {{ leaked into JS");
    assert!(!js.contains("}}"), "literal }} leaked into JS");
}

#[test]
fn surface_js_leaves_play_and_pause_to_native_media_session_handlers() {
    let js = surface_js("[]", "0", "1", "null");
    assert!(js.contains("setH('play', null);"));
    assert!(js.contains("setH('pause', null);"));
    assert!(!js.contains("setH('play', function"));
    assert!(!js.contains("setH('pause', function"));
    assert!(js.contains("setH('seekbackward', function"));
    assert!(js.contains("setH('seekforward', function"));
    assert!(js.contains("setH('seekto', function"));
}

#[test]
fn marquee_js_measures_only_the_primary_span_and_toggles_the_overflow_class() {
    assert!(MARQUEE_JS.contains(".m-player-title-track"));
    assert!(MARQUEE_JS.contains("track.children[0]"));
    assert!(MARQUEE_JS.contains("primary.scrollWidth > h1.clientWidth"));
    assert!(MARQUEE_JS.contains("classList.toggle('is-overflowing', overflowing)"));
}

#[test]
fn now_playing_json_lit_sets_album_omnibus_and_artwork() {
    let lit = NowPlaying {
        title: "T",
        author: "A",
        artwork_url: Some("http://h/c?token=t"),
    }
    .to_json_lit();
    let v: serde_json::Value = serde_json::from_str(&lit).unwrap();
    assert_eq!(v["title"], "T");
    assert_eq!(v["artist"], "A");
    assert_eq!(v["album"], "Omnibus");
    assert_eq!(v["artwork"], "http://h/c?token=t");
}

#[test]
fn audio_event_time_deserializes_with_paused_flag() {
    let ev: AudioEvent =
        serde_json::from_str(r#"{"kind":"Time","seconds":42.5,"paused":false}"#).unwrap();
    match ev {
        AudioEvent::Time { seconds, paused } => {
            assert_eq!(seconds, 42.5);
            assert!(!paused);
        }
        other => panic!("expected Time, got {other:?}"),
    }
}

#[test]
fn now_playing_json_lit_null_artwork_when_absent() {
    let lit = NowPlaying {
        title: "T",
        author: "A",
        artwork_url: None,
    }
    .to_json_lit();
    let v: serde_json::Value = serde_json::from_str(&lit).unwrap();
    assert!(v["artwork"].is_null());
}
