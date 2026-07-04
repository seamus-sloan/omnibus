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

/// Install the direct-play control surface and return the persistent [`Eval`]
/// the caller drains for [`AudioEvent`]s. Part URLs are tokened here so the
/// `<audio src>` fetch authenticates on mobile.
pub fn install_direct_surface(
    server_url: &str,
    parts: &[ManifestPart],
    resume_seconds: f64,
    rate: f64,
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
    dioxus::document::eval(&surface_js(&parts_json, &resume_lit, &rate_lit))
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

/// Set the playback rate.
pub fn set_rate(rate: f64) {
    let lit = serde_json::to_string(&rate).unwrap_or_else(|_| "1".into());
    fire(&format!(
        "window.OmnibusMobileAudio && window.OmnibusMobileAudio.setRate({lit});"
    ));
}

/// Fire-and-forget control eval (no event stream).
fn fire(js: &str) {
    let _ = dioxus::document::eval(js);
}

/// Build the `window.OmnibusMobileAudio` install script. Only the three
/// interpolation points (`parts_json`, `resume_lit`, `rate_lit`) use
/// `format!`; the rest is literal JS.
fn surface_js(parts_json: &str, resume_lit: &str, rate_lit: &str) -> String {
    format!(
        r#"
(function(){{
  var parts = {parts_json};
  var resume = {resume_lit};
  var rate = {rate_lit};

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

  function absTime() {{
    var oa = window.OmnibusMobileAudio;
    if (!oa) return el.currentTime || 0;
    return (oa._offsets[oa._index] || 0) + (el.currentTime || 0);
  }}

  window.OmnibusMobileAudio = {{
    _parts: parts,
    _offsets: offsets,
    _index: 0,
    play: function(){{ var p = el.play(); if (p && p.catch) p.catch(function(){{}}); }},
    pause: function(){{ el.pause(); }},
    toggle: function(){{ if (el.paused) {{ this.play(); }} else {{ this.pause(); }} }},
    setRate: function(r){{ try {{ el.playbackRate = r; }} catch(_e) {{}} }},
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

  el.addEventListener('timeupdate', function(){{ dioxus.send({{ kind: 'Time', seconds: absTime() }}); }});
  el.addEventListener('play',  function(){{ dioxus.send({{ kind: 'Play' }}); }});
  el.addEventListener('pause', function(){{ dioxus.send({{ kind: 'Pause', seconds: absTime() }}); }});
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
        let js = surface_js("[{\"url\":\"u\",\"duration\":1}]", "12.5", "1.2");
        assert!(js.contains("var resume = 12.5;"));
        assert!(js.contains("el.playbackRate = rate;"));
        assert!(js.contains("window.OmnibusMobileAudio"));
        assert!(js.contains("dioxus.send"));
        // No stray `format!` escape pairs leaked into the emitted JS.
        assert!(!js.contains("{{"), "literal {{ leaked into JS");
        assert!(!js.contains("}}"), "literal }} leaked into JS");
    }
}
