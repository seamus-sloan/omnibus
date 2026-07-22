//! Mobile `<audio>` control surface via `dioxus::document::eval`.
//!
//! Mobile is a wry WebView, so an HTML `<audio>` element plays natively. We
//! install a `window.OmnibusMobileAudio` object once (part list, cumulative
//! offsets, transport methods, cross-part `ended` auto-advance) and push
//! `timeupdate` / `play` / `pause` events back to Rust over the Dioxus
//! `eval` channel (`dioxus.send(...)` → `Eval::recv().await`). Control
//! commands (toggle / seek / skip / setRate) are fire-and-forget evals.
//!
//! Everything here is a JS-interop seam; the testable arithmetic lives in
//! [`super::view`]. The pure part-URL builder ([`super::view::part_token_url`])
//! is exercised by that module's tests.

use dioxus::document::Eval;
use omnibus_shared::ManifestPart;

use super::view::part_token_url;

/// One event pushed up from the JS audio element.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum AudioEvent {
    /// Current absolute (cross-part) position, in seconds.
    Time { seconds: f64 },
    /// Playback started.
    Play,
    /// Playback paused (carries the position at pause).
    Pause { seconds: f64 },
}

/// Now-playing metadata for the iOS lock screen / Control Center, pushed
/// through the Media Session API. WKWebView mirrors `navigator.mediaSession`
/// into `MPNowPlayingInfoCenter`, so this is what surfaces the book cover +
/// "Omnibus" branding during background playback instead of a blank widget.
pub struct NowPlaying<'a> {
    pub title: &'a str,
    pub author: &'a str,
    /// Absolute, token-bearing cover URL. WebKit fetches the artwork itself,
    /// so it must carry the `?token=` query — a system fetch can't send a
    /// bearer header. `None` when the book has no cover.
    pub artwork_url: Option<&'a str>,
}

impl NowPlaying<'_> {
    /// Serialize to the `{title, artist, album, artwork}` JS literal the
    /// surface script feeds to `MediaMetadata`.
    fn to_json_lit(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "title": self.title,
            "artist": self.author,
            "album": "Omnibus",
            "artwork": self.artwork_url,
        }))
        .unwrap_or_else(|_| "null".into())
    }
}

/// Install the direct-play control surface and return the persistent [`Eval`]
/// the caller drains for [`AudioEvent`]s. Part URLs are tokened here so the
/// `<audio src>` fetch authenticates on mobile; `now_playing` seeds the
/// lock-screen Media Session metadata.
pub fn install_direct_surface(
    server_url: &str,
    parts: &[ManifestPart],
    resume_seconds: f64,
    rate: f64,
    now_playing: &NowPlaying,
) -> Eval {
    let token = crate::data::token_store::get();
    let tokened: Vec<serde_json::Value> = parts
        .iter()
        .map(|p| {
            serde_json::json!({
                "url": part_token_url(server_url, &p.url, token.as_deref()),
                "duration": p.duration_seconds,
            })
        })
        .collect();
    let parts_json = serde_json::to_string(&tokened).unwrap_or_else(|_| "[]".into());
    let resume_lit = serde_json::to_string(&resume_seconds).unwrap_or_else(|_| "0".into());
    let rate_lit = serde_json::to_string(&rate).unwrap_or_else(|_| "1".into());
    let meta_lit = now_playing.to_json_lit();
    dioxus::document::eval(&surface_js(&parts_json, &resume_lit, &rate_lit, &meta_lit))
}

/// Toggle play/pause on the installed surface.
pub fn toggle() {
    fire("window.OmnibusMobileAudio && window.OmnibusMobileAudio.toggle();");
}

/// Seek to an absolute (cross-part) second offset.
pub fn seek(seconds: f64) {
    let lit = serde_json::to_string(&seconds).unwrap_or_else(|_| "0".into());
    fire(&format!(
        "window.OmnibusMobileAudio && window.OmnibusMobileAudio.seek({lit});"
    ));
}

/// Relative skip (+30 / -30), computed in absolute terms.
pub fn skip(delta: f64) {
    let lit = serde_json::to_string(&delta).unwrap_or_else(|_| "0".into());
    fire(&format!(
        "window.OmnibusMobileAudio && window.OmnibusMobileAudio.skip({lit});"
    ));
}

/// Pause playback (sleep-timer expiry — the UI toggle goes through
/// [`toggle`]).
pub fn pause() {
    fire("window.OmnibusMobileAudio && window.OmnibusMobileAudio.pause();");
}

/// Remove the audio element and the control surface entirely (playback
/// dismissed, not just paused).
pub fn teardown() {
    fire(
        "var el = document.getElementById('m-omnibus-audio'); \
         if (el) { try { el.pause(); } catch(_e) {} el.remove(); } \
         window.OmnibusMobileAudio = null; \
         if ('mediaSession' in navigator) { \
           try { \
             navigator.mediaSession.metadata = null; \
             navigator.mediaSession.playbackState = 'none'; \
             ['play','pause','seekbackward','seekforward','seekto'].forEach(function(a){ \
               try { navigator.mediaSession.setActionHandler(a, null); } catch(_e) {} \
             }); \
           } catch(_e) {} \
         }",
    );
}

/// Set the playback rate.
pub fn set_rate(rate: f64) {
    let lit = serde_json::to_string(&rate).unwrap_or_else(|_| "1".into());
    fire(&format!(
        "window.OmnibusMobileAudio && window.OmnibusMobileAudio.setRate({lit});"
    ));
}

/// Re-measure the `.m-player-title` track against its container and toggle
/// the marquee animation on only when the title actually overflows — a
/// short title stays static. Fire-and-forget; call after the title text
/// mounts or changes.
pub fn refresh_title_marquee() {
    fire(MARQUEE_JS);
}

/// Measures only the primary (first, always-visible) track span — the
/// second copy exists solely for the animated loop, and CSS gives it a
/// `gap` for loop spacing, so including it in the measurement would flag a
/// title that actually fits as overflowing. Double `requestAnimationFrame`
/// lets layout (webfonts in particular) settle before measuring
/// `scrollWidth`.
const MARQUEE_JS: &str = "requestAnimationFrame(function(){ requestAnimationFrame(function(){ \
     document.querySelectorAll('.m-player-title').forEach(function(h1){ \
       var track = h1.querySelector('.m-player-title-track'); \
       var primary = track && track.children[0]; \
       if (!primary) return; \
       var overflowing = primary.scrollWidth > h1.clientWidth; \
       h1.classList.toggle('is-overflowing', overflowing); \
     }); \
   }); });";

/// Fire-and-forget control eval (no event stream).
fn fire(js: &str) {
    let _ = dioxus::document::eval(js);
}

/// Build the `window.OmnibusMobileAudio` install script. Only the four
/// interpolation points (`parts_json`, `resume_lit`, `rate_lit`, `meta_lit`)
/// use `format!`; the rest is literal JS.
fn surface_js(parts_json: &str, resume_lit: &str, rate_lit: &str, meta_lit: &str) -> String {
    format!(
        r#"
(function(){{
  var parts = {parts_json};
  var resume = {resume_lit};
  var rate = {rate_lit};
  var meta = {meta_lit};

  // Cumulative per-part offsets for the absolute (cross-part) timeline.
  var offsets = [];
  var acc = 0;
  for (var i = 0; i < parts.length; i++) {{ offsets.push(acc); acc += (parts[i].duration || 0); }}

  // Drop any prior element before reinstalling: reusing it would stack a fresh
  // set of timeupdate/play/pause/ended listeners (and orphan the old Eval
  // channel's dioxus.send closures) on every SPA re-entry / book switch.
  var old = document.getElementById('m-omnibus-audio');
  if (old) {{ try {{ old.pause(); }} catch(_e) {{}} old.remove(); }}
  var el = document.createElement('audio');
  el.id = 'm-omnibus-audio';
  el.preload = 'auto';
  el.style.display = 'none';
  document.body.appendChild(el);
  try {{ el.playbackRate = rate; }} catch(_e) {{}}

  // WKWebView resets `playbackRate` to 1.0 every time a new resource
  // loads, so the rate set above (and every `setRate`) is wiped by the
  // seed / cross-part / auto-advance `el.load()` calls below — the book
  // plays at 1.0 while the UI shows the saved speed. Re-apply the tracked
  // rate on every `loadedmetadata` to keep the element in sync.
  el.addEventListener('loadedmetadata', function(){{
    var oa = window.OmnibusMobileAudio;
    if (oa) {{ try {{ el.playbackRate = oa._rate; }} catch(_e) {{}} }}
  }});

  function absTime() {{
    var oa = window.OmnibusMobileAudio;
    if (!oa) return el.currentTime || 0;
    return (oa._offsets[oa._index] || 0) + (el.currentTime || 0);
  }}

  window.OmnibusMobileAudio = {{
    _parts: parts,
    _offsets: offsets,
    _index: 0,
    // Tracked separately because WKWebView drops `el.playbackRate` to 1.0
    // on every resource load; the `loadedmetadata` listener re-applies it.
    _rate: rate,
    play: function(){{ var p = el.play(); if (p && p.catch) p.catch(function(){{}}); }},
    pause: function(){{ el.pause(); }},
    toggle: function(){{ if (el.paused) {{ this.play(); }} else {{ this.pause(); }} }},
    setRate: function(r){{ this._rate = r; try {{ el.playbackRate = r; }} catch(_e) {{}} }},
    seek: function(absSeconds){{
      var s = Math.max(0, absSeconds || 0);
      var i = 0;
      while (i < this._offsets.length - 1 && s >= this._offsets[i + 1]) i++;
      var local = s - this._offsets[i];
      if (i !== this._index) {{
        var wasPlaying = !el.paused;
        this._index = i;
        var onMeta = function(){{
          el.removeEventListener('loadedmetadata', onMeta);
          try {{ el.currentTime = local; }} catch(_e) {{}}
          if (wasPlaying) {{ var p = el.play(); if (p && p.catch) p.catch(function(){{}}); }}
        }};
        el.addEventListener('loadedmetadata', onMeta);
        el.src = this._parts[i].url; el.load();
      }} else {{
        try {{ el.currentTime = local; }} catch(_e) {{}}
      }}
    }},
    skip: function(d){{ this.seek(absTime() + d); }},
  }};

  // iOS lock-screen / Control Center now-playing via the Media Session API.
  // WKWebView mirrors this into MPNowPlayingInfoCenter, so it's what shows the
  // book cover + "Omnibus" instead of blank branding. Guarded — older WebViews
  // lack the API.
  var hasMediaSession = ('mediaSession' in navigator);
  function updatePositionState() {{
    if (!hasMediaSession || !navigator.mediaSession.setPositionState) return;
    if (!(acc > 0) || !isFinite(acc)) return;
    try {{
      navigator.mediaSession.setPositionState({{
        duration: acc,
        playbackRate: el.playbackRate || 1,
        position: Math.max(0, Math.min(absTime(), acc)),
      }});
    }} catch(_e) {{}}
  }}
  if (hasMediaSession) {{
    try {{
      var art = (meta && meta.artwork)
        ? [{{ src: meta.artwork, sizes: '512x512', type: 'image/webp' }}]
        : [];
      navigator.mediaSession.metadata = new MediaMetadata({{
        title: (meta && meta.title) || '',
        artist: (meta && meta.artist) || '',
        album: (meta && meta.album) || 'Omnibus',
        artwork: art,
      }});
    }} catch(_e) {{}}
    var oa = window.OmnibusMobileAudio;
    var setH = function(a, fn){{ try {{ navigator.mediaSession.setActionHandler(a, fn); }} catch(_e) {{}} }};
    // WebKit's native handlers keep working while backgrounded JS is suspended.
    setH('play', null);
    setH('pause', null);
    setH('seekbackward', function(d){{ oa.skip(-((d && d.seekOffset) || 30)); }});
    setH('seekforward', function(d){{ oa.skip((d && d.seekOffset) || 30); }});
    setH('seekto', function(d){{ if (d && d.seekTime != null) oa.seek(d.seekTime); }});
  }}

  el.addEventListener('timeupdate', function(){{ dioxus.send({{ kind: 'Time', seconds: absTime() }}); updatePositionState(); }});
  el.addEventListener('play',  function(){{ dioxus.send({{ kind: 'Play' }}); if (hasMediaSession) navigator.mediaSession.playbackState = 'playing'; updatePositionState(); }});
  el.addEventListener('pause', function(){{ dioxus.send({{ kind: 'Pause', seconds: absTime() }}); if (hasMediaSession) navigator.mediaSession.playbackState = 'paused'; }});
  // Cross-part auto-advance: chain to the next part on natural end.
  el.addEventListener('ended', function(){{
    var oa = window.OmnibusMobileAudio;
    if (!oa) return;
    if (oa._index + 1 < oa._parts.length) {{
      oa._index += 1;
      el.src = oa._parts[oa._index].url; el.load();
      var p = el.play(); if (p && p.catch) p.catch(function(){{}});
    }}
  }});

  // Seed the starting part for the resume position.
  (function(){{
    var s = Math.max(0, resume);
    var idx = 0;
    while (idx < offsets.length - 1 && s >= offsets[idx + 1]) idx++;
    window.OmnibusMobileAudio._index = idx;
    var local = s - offsets[idx];
    var onMeta = function(){{
      el.removeEventListener('loadedmetadata', onMeta);
      try {{ el.currentTime = local; }} catch(_e) {{}}
    }};
    el.addEventListener('loadedmetadata', onMeta);
    if (parts[idx]) {{ el.src = parts[idx].url; el.load(); }}
  }})();
}})();
"#
    )
}

#[cfg(test)]
mod tests {
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
}
