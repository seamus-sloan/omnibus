//! Pure JS-string composition for the `window.OmnibusAudio` control
//! surface: the IIFE scaffold, transport controls, DOM reset, element
//! listeners, the direct-play / HLS init bodies, and the hls.js script
//! injection. [`super`] owns the Rust-side lifecycle and calls into here.

use super::super::helpers::HLS_JS;

/// Build the `window.OmnibusAudio` IIFE script (one self-contained JS module).
///
/// Composes four sub-segments: a DOM-reset/listener block (with the
/// initial playback rate and volume interpolated in), the in-line
/// OmnibusAudio object scaffold (controls + state + the book uuid), and
/// the two pure JS init methods for direct-play and HLS. Mismatched brace
/// escapes in the `format!` args would silently break audio playback, so
/// each pure JS segment lives in its own helper as a raw `&'static str`
/// (literal `{`/`}`, no escaping required) and only `control_surface_js`
/// itself uses `format!` for the three interpolation points.
pub(super) fn control_surface_js(rate_lit: &str, vol_lit: &str, uuid_lit: &str) -> String {
    format!(
        r#"
(function(){{
{dom_reset}
    el.playbackRate = {rate_lit};
    el.volume = {vol_lit};
{listeners}

    window.OmnibusAudio = {{
{transport}
{direct_init}

{hls_init}

      _uuid: {uuid_lit},
      _rate: {rate_lit},
    }};
  }}
  mount();
}})();
"#,
        dom_reset = dom_reset_js(),
        listeners = listeners_js(),
        transport = transport_controls_js(),
        direct_init = direct_play_init_js(),
        hls_init = hls_init_js(),
    )
}

/// OmnibusAudio object scaffold: mode + direct-mode state plus the
/// transport-control methods (play/pause/toggle/setRate/stop/seek/skip).
/// `seek` pushes the target time to the transport display synchronously
/// (not just via the element's `timeupdate`), so a paused seek shows
/// immediate feedback (#1897). Pure JS with no Rust interpolation, so it
/// lives as a raw `&'static str` with literal braces.
fn transport_controls_js() -> &'static str {
    r#"      // Playback mode. Set by initDirect / initHls; null before either fires.
      _mode: null,
      // Direct-mode state — null in HLS mode.
      _parts: null,
      _index: 0,
      _cumOffsets: [],
      _totalDuration: 0,

      play:    function(){ var p = el.play(); if (p && p.catch) p.catch(function(){}); },
      pause:   function(){ el.pause(); },
      toggle:  function(){ if (el.paused) { this.play(); } else { this.pause(); } },
      // Track the rate on the shim as well as the element: a media `load()`
      // (new src / part swap / HLS attach) resets `el.playbackRate` to 1.0,
      // and the `loadedmetadata` listener re-applies this tracked value.
      setRate: function(r){ try { el.playbackRate = r; this._rate = r; } catch(_) {} },
      // User volume slider + sleep-timer fade both drive this. Clamps to
      // [0,1]; the Rust countdown ramps it down over the final seconds and
      // restores the user's chosen volume (not always 1.0) on cancel/expiry.
      setVolume: function(v){ try { el.volume = Math.max(0, Math.min(1, v)); } catch(_) {} },
      // Read path for the volume slider's initial-mount sync. The Rust side
      // tracks the target volume in `PlaybackState.volume`, so this mainly
      // exists for symmetry with `setVolume`.
      getVolume: function(){ try { return el.volume; } catch(_) { return 1; } },

      // Hard stop for the dock's dismiss: pause, drop the source so a
      // media-key resume can't restart it, and reset direct-mode state.
      stop:    function(){
        try { el.pause(); } catch(_) {}
        try { el.removeAttribute('src'); el.load(); } catch(_) {}
        this._mode = null; this._parts = null; this._index = 0;
        this._cumOffsets = []; this._totalDuration = 0;
      },

      // Seek to an absolute (cross-part) second offset. For direct mode
      // this finds the target part by cumulative duration and switches
      // `el.src` if it differs from the current part, preserving the
      // play/pause state across the swap.
      seek: function(absSeconds){
        this._seeded = true;
        this._lastAbs = Math.max(0, absSeconds || 0);
        var s = Math.max(0, absSeconds || 0);
        // Push the target time to the transport display immediately —
        // don't wait for the element's own `timeupdate`, which some
        // browsers fire unreliably (or not at all) for a seek issued while
        // paused, leaving the readout and scrub position stuck at the old
        // value until playback starts (#1897).
        if (typeof window.__omnibusOnAudioTime === 'function') { window.__omnibusOnAudioTime(s); }
        if (this._mode === 'direct' && this._parts) {
          var i = 0;
          while (i < this._cumOffsets.length - 1 && s >= this._cumOffsets[i + 1]) i++;
          var local = s - this._cumOffsets[i];
          if (i !== this._index) {
            var wasPlaying = !el.paused;
            this._index = i;
            var onMeta = function(){
              el.removeEventListener('loadedmetadata', onMeta);
              try { el.currentTime = local; } catch(_) {}
              if (wasPlaying) {
                var p = el.play(); if (p && p.catch) p.catch(function(){});
              }
            };
            el.addEventListener('loadedmetadata', onMeta);
            el.src = this._parts[i].url;
            el.load();
          } else {
            try { el.currentTime = local; } catch(_) {}
          }
        } else {
          try { el.currentTime = s; } catch(_) {}
        }
      },

      // Relative skip (+30 / -30). Computed in absolute terms so a skip
      // straddling a part boundary works correctly.
      skip: function(d){ this.seek(absTime() + d); },
"#
}

/// IIFE prologue + element reset: clears any stale `window.OmnibusAudio`
/// captured by an SPA-nav from the previous visit, polls for the audio
/// element, and resets its src/preload. Stops just before
/// `el.playbackRate` so the composer can interpolate the initial rate.
fn dom_reset_js() -> &'static str {
    r#"  // SPA-nav from another page leaves a stale `window.OmnibusAudio` from
  // the previous visit, captured in a closure over a now-detached
  // `<audio>` element. The init poll below sees that stale object,
  // calls `initDirect` on it, and the visible audio element never gets
  // a src — the scrub bar reads 0:00 until a full reload. Clearing
  // here forces the init poll to wait for the fresh install.
  try { var _prev = window.OmnibusAudio; if (_prev) { _prev._stale = true; } } catch(_) {}
  window.OmnibusAudio = null;
  // Wait for the audio element to appear in the DOM.
  var n = 0;
  function mount(){
    var el = document.getElementById('omnibus-audio');
    if (!el) { if (n++ < 200) { return setTimeout(mount, 50); } else { return; } }
    // Reset the element so leftover src / preloading from a prior mount
    // doesn't keep streaming once we swap modes.
    try { el.pause(); } catch(_) {}
    el.removeAttribute('src');
    el.preload = 'auto';"#
}

/// `absTime()` helper and the audio-element event listeners
/// (loadedmetadata / timeupdate / play / pause / ended). Pure JS with no
/// Rust interpolation, so it lives as a raw `&'static str` (literal
/// braces, no escaping).
fn listeners_js() -> &'static str {
    r#"
    // Helper: absolute seconds across the whole book. For direct mode
    // this adds the current part's cumulative offset; for HLS the audio
    // element's `currentTime` already IS absolute (one continuous
    // timeline).
    function absTime() {
      var oa = window.OmnibusAudio;
      if (oa && oa._mode === 'direct' && oa._parts) {
        return (oa._cumOffsets[oa._index] || 0) + (el.currentTime || 0);
      }
      return el.currentTime || 0;
    }

    el.addEventListener('loadedmetadata', function(){
      // Initial seek is the job of init{Direct,Hls} via their own
      // one-shot loadedmetadata listeners — this listener only reports
      // duration. For direct mode we always report the book-level total
      // so a part change does not collapse the scrub bar to per-part.
      var oa = window.OmnibusAudio;
      // A media `load()` resets playbackRate to defaultPlaybackRate (1.0),
      // so re-apply the tracked rate on every load — otherwise a restored
      // speed shows in the UI but plays at 1.0 until the user re-picks it.
      if (oa && typeof oa._rate === 'number') {
        try { el.playbackRate = oa._rate; } catch(_) {}
      }
      if (window.__omnibusOnAudioDuration) {
        var d = (oa && oa._mode === 'direct' && oa._totalDuration > 0)
          ? oa._totalDuration
          : (el.duration || 0);
        window.__omnibusOnAudioDuration(d);
      }
    });
    // The seed gate: until the boot's initial position has actually been
    // applied to the element (or the user seeks/plays for real), the
    // element sits at 0 — and a pause fired by a load()/src swap in that
    // window would flush a hard 0.0 over the stored position with a fresh
    // clock, which the cross-format follow then propagates into the other
    // format. No emission may persist an unseeded element's position.
    // Fail CLOSED when the shim is null (dom_reset_js clears it during
    // SPA-nav): a stale element's listeners can still fire in that window,
    // and an unowned element is exactly the unseeded-position source this
    // gate exists to silence.
    el.addEventListener('timeupdate', function(){
      var oa = window.OmnibusAudio;
      if (!oa || !oa._seeded) return;
      // Last position this session really reached — the teardown-artifact
      // guard on `pause` below compares against it.
      oa._lastAbs = absTime();
      if (window.__omnibusOnAudioTime) {
        window.__omnibusOnAudioTime(absTime());
      }
    });
    // A full-page navigation destroys the media element mid-session; the
    // browser's teardown can zero currentTime BEFORE the final pause event
    // dispatches, and that dying flush overwrote a real 2h48m position
    // with 0.0 live (#1954). The heartbeat persisted a ≤5s-stale position
    // moments earlier, so closing the gate loses nothing meaningful.
    window.addEventListener('pagehide', function(){
      var oa = window.OmnibusAudio;
      if (oa) oa._seeded = false;
    });
    el.addEventListener('play', function(){
      var oa = window.OmnibusAudio;
      if (!oa) return;
      oa._seeded = true;
      if (window.__omnibusOnAudioPlay) {
        window.__omnibusOnAudioPlay(absTime());
      }
    });
    el.addEventListener('pause', function(){
      var oa = window.OmnibusAudio;
      if (!oa || !oa._seeded) return;
      // Teardown artifact: a pause reporting ~0 when this session was
      // materially past it is the element being reset, not a listener
      // rewinding — a real jump-to-start arrives as a seek, which updates
      // _lastAbs through the ungated seek path before any pause fires.
      var t = absTime();
      if (t < 1 && typeof oa._lastAbs === 'number' && oa._lastAbs > 60) return;
      if (window.__omnibusOnAudioPause) {
        window.__omnibusOnAudioPause(t);
      }
    });
    // Cross-part advance — direct mode only. HLS treats the whole
    // book as one continuous stream so `ended` only fires at the
    // actual end (which we leave as-is so the UI naturally stops).
    el.addEventListener('ended', function(){
      var oa = window.OmnibusAudio;
      if (oa && oa._mode === 'direct' && oa._parts
          && oa._index + 1 < oa._parts.length) {
        oa._index += 1;
        el.src = oa._parts[oa._index].url;
        el.load();
        var p = el.play(); if (p && p.catch) p.catch(function(){});
        return;
      }
      // Nothing left to advance to: either the last part of a direct-mode
      // book just played out, or the single HLS stream did. Both mean every
      // file of the book has been listened through.
      if (window.__omnibusOnAudioEnded) {
        window.__omnibusOnAudioEnded(absTime());
      }
    });"#
}

/// JS body for `initDirect: function(parts, initialPositionAbs)` — picks
/// the starting part by cumulative offset and wires a one-shot
/// `loadedmetadata` listener to seek into it. Pure JS; lives as a raw
/// `&'static str` with literal braces.
fn direct_play_init_js() -> &'static str {
    r#"      // Direct-play init: parts is the array from the manifest endpoint
      // (`[{ordinal, url, duration_seconds, mime}]`), initialPositionAbs
      // is the absolute resume position in seconds.
      // `initialPositionAbs` may be null: an unattributable resume (the
      // row's seconds live in another file) boots at the top of the file
      // WITHOUT seeding — the transport's gate stays closed, so nothing
      // can persist this element's position until a real play or seek.
      initDirect: function(parts, initialPositionAbs){
        this._mode = 'direct';
        this._seeded = false;
        this._parts = parts;
        var acc = 0;
        this._cumOffsets = [];
        for (var i = 0; i < parts.length; i++) {
          this._cumOffsets.push(acc);
          acc += parts[i].duration_seconds || 0;
        }
        this._totalDuration = acc;
        // Push the total duration up to Rust eagerly so the scrub bar
        // gets the right max before the first part's metadata loads.
        if (window.__omnibusOnAudioDuration) {
          window.__omnibusOnAudioDuration(this._totalDuration);
        }
        // Pick the starting part for the resume position.
        var seedKnown = typeof initialPositionAbs === 'number';
        var s = Math.max(0, seedKnown ? initialPositionAbs : 0);
        var idx = 0;
        while (idx < this._cumOffsets.length - 1 && s >= this._cumOffsets[idx + 1]) idx++;
        this._index = idx;
        var local = s - this._cumOffsets[idx];
        var oa = this;
        var onMeta = function(){
          el.removeEventListener('loadedmetadata', onMeta);
          if (seedKnown) {
            try { el.currentTime = local; } catch(_) {}
            oa._seeded = true;
          }
        };
        el.addEventListener('loadedmetadata', onMeta);
        el.src = parts[idx].url;
        el.load();
      },"#
}

/// JS body for `initHls: function(url, initialPositionAbs)` — legacy
/// fallback for codecs the browser cannot play natively. Tries Hls.js,
/// then Safari/iOS native HLS, then warns. Pure JS; lives as a raw
/// `&'static str` with literal braces.
fn hls_init_js() -> &'static str {
    r#"      // HLS init: legacy fallback path for codecs the browser does not
      // play natively. `initialPositionAbs` is optional — Rust passes
      // `null` to skip the seek.
      initHls: function(url, initialPositionAbs){
        this._mode = 'hls';
        this._seeded = false;
        if (typeof Hls !== 'undefined' && Hls.isSupported()) {
          var hls = new Hls();
          hls.loadSource(url);
          hls.attachMedia(el);
          hls.on(Hls.Events.ERROR, function(_, d) {
            if (d.fatal && window.__omnibusOnAudioPause) {
              window.__omnibusOnAudioPause(el.currentTime || 0);
            }
          });
        } else if (el.canPlayType('application/vnd.apple.mpegurl')) {
          // Safari / iOS native HLS.
          el.src = url;
          el.load();
        } else {
          console.warn('OmnibusAudio: no HLS support in this browser');
        }
        if (typeof initialPositionAbs === 'number' && initialPositionAbs <= 0) {
          // An explicit zero is a real position (a fresh book boots as
          // number 0). JSON `null` is an UNATTRIBUTABLE resume — the
          // row's seconds live in another file — and must stay unseeded,
          // exactly like initDirect, or the HLS arm re-opens the
          // zero-wipe path (#1954).
          this._seeded = true;
        }
        if (typeof initialPositionAbs === 'number' && initialPositionAbs > 0) {
          var hoa = this;
          var onMeta = function(){
            el.removeEventListener('loadedmetadata', onMeta);
            try { el.currentTime = Math.max(0, initialPositionAbs); } catch(_) {}
            hoa._seeded = true;
          };
          el.addEventListener('loadedmetadata', onMeta);
        }
      },"#
}

/// Inject the vendored hls.js bundle once. The script tag is idempotent
/// because the browser caches it by URL.
pub(super) fn inject_hls_script() {
    let hls_src = serde_json::to_string(&HLS_JS.to_string()).unwrap_or_else(|_| "\"\"".into());
    let inject_js = format!(
        r#"(function(){{
            if (window.Hls) return;
            var s = document.createElement('script');
            s.src = {hls_src};
            s.async = true;
            document.head.appendChild(s);
        }})();"#
    );
    let _ = dioxus::document::eval(&inject_js);
}

/// Once the HLS transcode reports `ready`, eval the JS that polls for
/// `window.OmnibusAudio` and calls `initHls`. Same mount-race shield as
/// the Direct arm — `__omnibusOnInitTimeout` surfaces a UI failure if
/// the shim never installs.
pub(super) fn eval_hls_init(playlist_lit: &str, pos_lit: &str) {
    let init_js = format!(
        r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initHls({playlist_lit}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else {{ console.error('OmnibusAudio never installed; HLS init timed out'); if (typeof window.__omnibusOnInitTimeout === 'function') {{ window.__omnibusOnInitTimeout(0); }} }} }})(); }})();"#
    );
    let _ = dioxus::document::eval(&init_js);
}

// Only `control_surface_js` composition is testable natively; the two eval
// seams (`inject_hls_script`, `eval_hls_init`) need a WASM runtime and are
// covered by Playwright at ui_tests/playwright/tests/flows/.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_surface_js_contains_expected_segments() {
        let js = control_surface_js("1.25", "0.6", "\"abc-123\"");
        // Rust-side interpolation points landed.
        assert!(js.contains("el.playbackRate = 1.25;"));
        assert!(js.contains("el.volume = 0.6;"));
        assert!(js.contains("_uuid: \"abc-123\","));
        // The shim tracks the rate so a media `load()` reset can be undone.
        assert!(js.contains("_rate: 1.25,"), "initial _rate seed missing");
        assert!(
            js.contains("el.playbackRate = oa._rate;"),
            "loadedmetadata rate re-apply missing"
        );
        assert!(
            js.contains("this._rate = r;"),
            "setRate does not track _rate"
        );
        // Each pure JS segment contributed its signature substring.
        assert!(
            js.contains("SPA-nav from another page"),
            "dom_reset_js missing"
        );
        assert!(js.contains("function absTime()"), "listeners_js missing");
        assert!(
            js.contains("initDirect: function(parts"),
            "direct_play_init_js missing"
        );
        assert!(js.contains("initHls: function(url"), "hls_init_js missing");
        // A paused seek must push the transport display immediately (#1897)
        // rather than waiting on the element's own `timeupdate`.
        assert!(
            js.contains("typeof window.__omnibusOnAudioTime === 'function'"),
            "seek does not guard the immediate time push with a typeof check"
        );
        assert!(
            js.contains("__omnibusOnAudioTime(s)"),
            "seek does not push the target time immediately"
        );
        // Sanity: no stray `{{` / `}}` escape pairs leaked from a `format!`.
        assert!(!js.contains("{{"), "literal {{ leaked into JS");
        assert!(!js.contains("}}"), "literal }} leaked into JS");
    }
}
