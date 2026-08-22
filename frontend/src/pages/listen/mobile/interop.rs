//! Mobile `<audio>` control surface via `dioxus::document::eval`. Mobile is a
//! wry WebView, so an HTML `<audio>` element plays natively: a
//! `window.OmnibusMobileAudio` object is installed once and pushes `timeupdate`
//! / `play` / `pause` back to Rust over the Dioxus eval channel. Everything
//! here is a JS-interop seam; the testable arithmetic lives in [`super::view`].

use dioxus::document::Eval;
use omnibus_shared::ManifestPart;

use super::view::part_token_url;

/// One event pushed up from the JS audio element.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum AudioEvent {
    /// Current absolute (cross-part) position, in seconds, plus the element's
    /// authoritative `paused` state. The `paused` flag lets the drain
    /// reconcile the transport icon on every tick — WKWebView can resume after
    /// an audio interruption without re-dispatching a `play` event, so the
    /// discrete `Play`/`Pause` events alone can leave the button stuck.
    Time { seconds: f64, paused: bool },
    /// Playback started.
    Play,
    /// Playback paused (carries the position at pause).
    Pause { seconds: f64 },
    /// The last part played out with nothing left to advance to — every file
    /// of the book has been listened through.
    Ended,
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
        crate::js_interop::json_literal(&serde_json::json!({
            "title": self.title,
            "artist": self.author,
            "album": "Omnibus",
            "artwork": self.artwork_url,
        }))
    }
}

/// Install the direct-play control surface and return the persistent [`Eval`]
/// the caller drains for [`AudioEvent`]s. Parts of a completed offline
/// download play from the loopback media server; otherwise the URLs are
/// tokened so the `<audio src>` fetch authenticates against the real
/// server. `now_playing` seeds the lock-screen Media Session metadata.
pub fn install_direct_surface(
    server_url: &str,
    uuid: &str,
    parts: &[ManifestPart],
    resume_seconds: f64,
    rate: f64,
    now_playing: &NowPlaying,
) -> Eval {
    let token = crate::data::token_store::get();
    let tokened: Vec<serde_json::Value> = parts
        .iter()
        .map(|p| {
            let url = crate::offline::downloads::local_part_url(uuid, p.ordinal)
                .unwrap_or_else(|| part_token_url(server_url, &p.url, token.as_deref()));
            serde_json::json!({
                "url": url,
                "duration": p.duration_seconds,
            })
        })
        .collect();
    let parts_json = crate::js_interop::json_literal_or(&tokened, "[]");
    let resume_lit = crate::js_interop::json_literal_or(&resume_seconds, "0");
    let rate_lit = crate::js_interop::json_literal_or(&rate, "1");
    let meta_lit = now_playing.to_json_lit();
    dioxus::document::eval(&surface_js(&parts_json, &resume_lit, &rate_lit, &meta_lit))
}

/// Toggle play/pause on the installed surface.
pub fn toggle() {
    fire("window.OmnibusMobileAudio && window.OmnibusMobileAudio.toggle();");
}

/// Seek to an absolute (cross-part) second offset.
pub fn seek(seconds: f64) {
    let lit = crate::js_interop::json_literal_or(&seconds, "0");
    fire(&format!(
        "window.OmnibusMobileAudio && window.OmnibusMobileAudio.seek({lit});"
    ));
}

/// Relative skip (+30 / -30), computed in absolute terms.
pub fn skip(delta: f64) {
    let lit = crate::js_interop::json_literal_or(&delta, "0");
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
    let lit = crate::js_interop::json_literal_or(&rate, "1");
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

/// Build the `window.OmnibusMobileAudio` install script. Composes five
/// pure-JS sub-segments ([`mount_element_js`], [`transport_js`],
/// [`media_session_js`], [`listeners_js`], [`seed_resume_js`]) around the
/// four interpolation points (`parts_json`, `resume_lit`, `rate_lit`,
/// `meta_lit`) — the same composition idiom the desktop bootstrap's
/// `control_surface_js` uses for `dom_reset_js`/`listeners_js`/etc.
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

{mount}

{transport}

{media_session}

{listeners}

{seed}
}})();
"#,
        mount = mount_element_js(),
        transport = transport_js(),
        media_session = media_session_js(),
        listeners = listeners_js(),
        seed = seed_resume_js(),
    )
}

/// Create (or replace) the `<audio>` element, apply the initial rate, and
/// wire the `loadedmetadata` rate re-apply plus the `absTime()` helper.
/// Pure JS with no Rust interpolation (`rate` is already in scope from the
/// enclosing IIFE), so it lives as a raw `&'static str`.
fn mount_element_js() -> &'static str {
    r#"  // Drop any prior element before reinstalling: reusing it would stack a fresh
  // set of timeupdate/play/pause/ended listeners (and orphan the old Eval
  // channel's dioxus.send closures) on every SPA re-entry / book switch.
  var old = document.getElementById('m-omnibus-audio');
  if (old) { try { old.pause(); } catch(_e) {} old.remove(); }
  var el = document.createElement('audio');
  el.id = 'm-omnibus-audio';
  el.preload = 'auto';
  el.style.display = 'none';
  document.body.appendChild(el);
  try { el.playbackRate = rate; } catch(_e) {}

  // WKWebView resets `playbackRate` to 1.0 every time a new resource
  // loads, so the rate set above (and every `setRate`) is wiped by the
  // seed / cross-part / auto-advance `el.load()` calls below — the book
  // plays at 1.0 while the UI shows the saved speed. Re-apply the tracked
  // rate on every `loadedmetadata` to keep the element in sync.
  el.addEventListener('loadedmetadata', function(){
    var oa = window.OmnibusMobileAudio;
    if (oa) { try { el.playbackRate = oa._rate; } catch(_e) {} }
  });

  function absTime() {
    var oa = window.OmnibusMobileAudio;
    if (!oa) return el.currentTime || 0;
    return (oa._offsets[oa._index] || 0) + (el.currentTime || 0);
  }"#
}

/// The `window.OmnibusMobileAudio` transport object: state plus
/// play/pause/toggle/setRate/seek/skip. Pure JS with no Rust interpolation
/// (`parts`, `offsets`, `rate`, `el`, `absTime` are already in scope), so
/// it lives as a raw `&'static str`.
fn transport_js() -> &'static str {
    r#"  window.OmnibusMobileAudio = {
    _parts: parts,
    _offsets: offsets,
    _index: 0,
    // Tracked separately because WKWebView drops `el.playbackRate` to 1.0
    // on every resource load; the `loadedmetadata` listener re-applies it.
    _rate: rate,
    play: function(){ var p = el.play(); if (p && p.catch) p.catch(function(){}); },
    pause: function(){ el.pause(); },
    toggle: function(){ if (el.paused) { this.play(); } else { this.pause(); } },
    setRate: function(r){ this._rate = r; try { el.playbackRate = r; } catch(_e) {} },
    seek: function(absSeconds){
      var s = Math.max(0, absSeconds || 0);
      var i = 0;
      while (i < this._offsets.length - 1 && s >= this._offsets[i + 1]) i++;
      var local = s - this._offsets[i];
      if (i !== this._index) {
        var wasPlaying = !el.paused;
        this._index = i;
        var onMeta = function(){
          el.removeEventListener('loadedmetadata', onMeta);
          try { el.currentTime = local; } catch(_e) {}
          if (wasPlaying) { var p = el.play(); if (p && p.catch) p.catch(function(){}); }
        };
        el.addEventListener('loadedmetadata', onMeta);
        el.src = this._parts[i].url; el.load();
      } else {
        try { el.currentTime = local; } catch(_e) {}
      }
    },
    skip: function(d){ this.seek(absTime() + d); },
  };"#
}

/// The four audio-element event listeners (timeupdate/play/pause/ended)
/// that push [`AudioEvent`]s back to Rust and reconcile Media Session
/// state. Pure JS with no Rust interpolation (`el`, `absTime`,
/// `hasMediaSession`, `updatePositionState` come from
/// [`mount_element_js`]/[`media_session_js`]), so it lives as a raw
/// `&'static str`.
fn listeners_js() -> &'static str {
    r#"  el.addEventListener('timeupdate', function(){ dioxus.send({ kind: 'Time', seconds: absTime(), paused: el.paused }); updatePositionState(); });
  el.addEventListener('play',  function(){ dioxus.send({ kind: 'Play' }); if (hasMediaSession) navigator.mediaSession.playbackState = 'playing'; updatePositionState(); });
  el.addEventListener('pause', function(){ dioxus.send({ kind: 'Pause', seconds: absTime() }); if (hasMediaSession) navigator.mediaSession.playbackState = 'paused'; });
  // Cross-part auto-advance: chain to the next part on natural end.
  el.addEventListener('ended', function(){
    var oa = window.OmnibusMobileAudio;
    if (!oa) return;
    if (oa._index + 1 < oa._parts.length) {
      oa._index += 1;
      el.src = oa._parts[oa._index].url; el.load();
      var p = el.play(); if (p && p.catch) p.catch(function(){});
      return;
    }
    // Nothing left to advance to: the book is finished.
    dioxus.send({ kind: 'Ended' });
  });"#
}

/// Seed the starting part and in-part offset for the resume position. Pure
/// JS with no Rust interpolation (`resume`, `offsets`, `parts`, `el` are
/// already in scope), so it lives as a raw `&'static str`.
fn seed_resume_js() -> &'static str {
    r#"  // Seed the starting part for the resume position.
  (function(){
    var s = Math.max(0, resume);
    var idx = 0;
    while (idx < offsets.length - 1 && s >= offsets[idx + 1]) idx++;
    window.OmnibusMobileAudio._index = idx;
    var local = s - offsets[idx];
    var onMeta = function(){
      el.removeEventListener('loadedmetadata', onMeta);
      try { el.currentTime = local; } catch(_e) {}
    };
    el.addEventListener('loadedmetadata', onMeta);
    if (parts[idx]) { el.src = parts[idx].url; el.load(); }
  })();"#
}

/// iOS lock-screen / Control Center now-playing via the Media Session API:
/// seeds the `MediaMetadata`, wires `setPositionState`, and installs the
/// seek/skip action handlers. WKWebView mirrors this into
/// `MPNowPlayingInfoCenter`. Guarded — older WebViews lack the API. Pure JS
/// with no Rust interpolation (`meta`, `acc`, `el`, `absTime` are already in
/// scope from the enclosing IIFE), so it lives as a raw `&'static str`
/// (literal braces, no escaping) — the same idiom the desktop bootstrap's
/// `dom_reset_js`/`listeners_js` use.
fn media_session_js() -> &'static str {
    r#"  var hasMediaSession = ('mediaSession' in navigator);
  function updatePositionState() {
    if (!hasMediaSession || !navigator.mediaSession.setPositionState) return;
    if (!(acc > 0) || !isFinite(acc)) return;
    try {
      navigator.mediaSession.setPositionState({
        duration: acc,
        playbackRate: el.playbackRate || 1,
        position: Math.max(0, Math.min(absTime(), acc)),
      });
    } catch(_e) {}
  }
  if (hasMediaSession) {
    try {
      var art = (meta && meta.artwork)
        ? [{ src: meta.artwork, sizes: '512x512', type: 'image/webp' }]
        : [];
      navigator.mediaSession.metadata = new MediaMetadata({
        title: (meta && meta.title) || '',
        artist: (meta && meta.artist) || '',
        album: (meta && meta.album) || 'Omnibus',
        artwork: art,
      });
    } catch(_e) {}
    var oa = window.OmnibusMobileAudio;
    var setH = function(a, fn){ try { navigator.mediaSession.setActionHandler(a, fn); } catch(_e) {} };
    // WebKit's native handlers keep working while backgrounded JS is suspended.
    setH('play', null);
    setH('pause', null);
    setH('seekbackward', function(d){ oa.skip(-((d && d.seekOffset) || 30)); });
    setH('seekforward', function(d){ oa.skip((d && d.seekOffset) || 30); });
    setH('seekto', function(d){ if (d && d.seekTime != null) oa.seek(d.seekTime); });
  }"#
}

#[cfg(test)]
mod tests;
