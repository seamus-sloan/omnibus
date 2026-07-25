/*
 * barcode-scanner-glue.js — Omnibus camera barcode bridge.
 *
 * Depends on one global that MUST be loaded BEFORE this file:
 *   - `ZXingWASM` (zxing-wasm@3.1.2, reader-only IIFE) — vendored at
 *     ./zxing-reader.min.js, with its 1 MB `zxing_reader.wasm` sibling. The
 *     pair is self-verifying: the bundle carries the wasm's expected sha256 as
 *     `ZXING_WASM_SHA256`
 *     (0e8d688d71932ebb6b8b33f700d43d3cb997f59ed9cab3c05102d7f10288a392), so
 *     `shasum -a 256 zxing_reader.wasm` proves the two came from one release.
 * The wasm URL is never guessed: zxing-wasm otherwise falls back to a jsDelivr
 * CDN, which mobile (bundled assets, no network to a CDN) can't reach. The Rust
 * side passes the hashed `asset!` URL through `opts.wasmUrl`.
 *
 * The Rust scanner sets three optional callbacks on `window`:
 *   - `__omnibusOnScanStatus(state)` — "starting" | "ready" | "denied" |
 *     "unsupported" | "error". Drives the permission / fallback UI.
 *   - `__omnibusOnScanTorch(flag)`  — "1" once the active track reports a
 *     torch capability, "0" otherwise. The toggle stays hidden on "0".
 *   - `__omnibusOnScanResult(text)` — the decoded 13-digit EAN. Fired once;
 *     the decode loop stops itself before firing.
 * Every callback takes a single string so the mobile `dioxus.send` shims are
 * uniform (see `check_in::scanner::interop`).
 *
 * Public surface: window.OmnibusScanner
 *   start(videoElementId, opts)  opts = { wasmUrl }
 *   stop()
 *   setTorch(on)
 */
(function () {
  "use strict";

  // Decode cadence. Scheduled *after* each decode settles, so a slow frame
  // throttles the loop instead of queueing more work behind itself.
  var FRAME_INTERVAL_MS = 180;
  // Longest edge of the frame handed to zxing. Measured against a real book
  // barcode: zxing needs roughly 2px per EAN module, so a 95-module symbol
  // needs ~200px and stops reading below that — and it *misreads* (a wrong but
  // checksum-valid EAN) right at the margin. A barcode is a small part of the
  // frame, so downscaling the capture is exactly the wrong economy; keep the
  // sensor's own pixels and let the crop below do the size reduction.
  var MAX_FRAME_WIDTH = 1920;
  // A barcode must decode to the same value on two consecutive frames before
  // we emit it. One bad frame on a glossy dust jacket can yield a plausible
  // but wrong EAN; two in a row effectively never do.
  var CONFIRMATIONS_REQUIRED = 2;
  // Fraction of the frame height handed to the decoder, centred. Deliberately
  // looser than the on-screen reticle: a band that hugs the reticle drops a
  // barcode the reader framed slightly low, and a 1D symbol only needs one
  // clean scan line through the bars.
  var BAND_HEIGHT = 0.7;
  // Consecutive decoder throws before we call it: a decode that fails on a
  // blurred frame is routine, but a decoder that throws on *every* frame is a
  // dead scanner, and silently looping on it is indistinguishable from "the
  // barcode just isn't being picked up".
  var MAX_CONSECUTIVE_ERRORS = 15;

  var state = {
    stream: null,
    track: null,
    video: null,
    canvas: null,
    ctx: null,
    timer: null,
    running: false,
    lastText: null,
    lastCount: 0,
    errors: 0,
  };

  function emit(name, value) {
    var fn = window[name];
    if (typeof fn === "function") {
      try {
        fn(value);
      } catch (e) {
        /* a throwing host callback must not kill the decode loop */
      }
    }
  }

  var status = function (s) {
    emit("__omnibusOnScanStatus", s);
  };

  /* ── EAN-13 / ISBN shaping ─────────────────────────────────────── */

  // zxing hands back the raw symbol text, which for a book barcode may carry a
  // 2- or 5-digit price add-on. Keep the leading 13 digits and require a
  // Bookland prefix — 978/979 is what a book's EAN-13 always starts with, so
  // this rejects the cereal box the camera happened to catch first.
  function isbnFrom(text) {
    var digits = String(text || "").replace(/\D/g, "");
    if (digits.length < 13) return null;
    var ean = digits.slice(0, 13);
    if (ean.slice(0, 3) !== "978" && ean.slice(0, 3) !== "979") return null;
    return checkDigitOk(ean) ? ean : null;
  }

  // Mod-10 with alternating 1/3 weights. zxing validates this itself, but the
  // add-on trim above re-slices the string, so verify what we actually emit.
  function checkDigitOk(ean) {
    var sum = 0;
    for (var i = 0; i < 13; i++) {
      sum += (ean.charCodeAt(i) - 48) * (i % 2 === 0 ? 1 : 3);
    }
    return sum % 10 === 0;
  }

  /* ── Camera ────────────────────────────────────────────────────── */

  function classifyError(err) {
    var name = (err && err.name) || "";
    if (name === "NotAllowedError" || name === "SecurityError") return "denied";
    if (name === "NotFoundError" || name === "OverconstrainedError") return "unsupported";
    return "error";
  }

  function openCamera() {
    // No mediaDevices at all means an insecure origin or an ancient WebView —
    // either way the manual keypad is the only way through, so report it as
    // "unsupported" rather than a transient error the user might retry.
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      return Promise.reject({ name: "NotFoundError" });
    }
    // Ask for far more than we expect: a barcode occupies a small slice of the
    // frame, and every camera clamps `ideal` to what it actually has, so this
    // costs nothing on a modest webcam and wins real resolution on a phone.
    return navigator.mediaDevices.getUserMedia({
      audio: false,
      video: {
        facingMode: { ideal: "environment" },
        width: { ideal: 3840 },
        height: { ideal: 2160 },
      },
    });
  }

  function reportTorch(track) {
    var caps = track && track.getCapabilities ? track.getCapabilities() : null;
    emit("__omnibusOnScanTorch", caps && "torch" in caps ? "1" : "0");
  }

  /* ── Decode loop ───────────────────────────────────────────────── */

  // Crop to the centred band of the frame — roughly what the viewfinder shows
  // — so the decoder spends its per-frame budget on the strip the barcode is
  // in rather than the whole scene. Full width is kept: the stage is
  // `object-fit: cover`, so it already hides the frame's left/right edges, and
  // cropping them again in here would silently narrow what can be read.
  function grabFrame() {
    var v = state.video;
    if (!v || !v.videoWidth || !v.videoHeight) return null;
    var scale = Math.min(1, MAX_FRAME_WIDTH / v.videoWidth);
    var srcY = v.videoHeight * ((1 - BAND_HEIGHT) / 2);
    var srcH = v.videoHeight * BAND_HEIGHT;
    var w = Math.round(v.videoWidth * scale);
    var h = Math.round(srcH * scale);
    if (state.canvas.width !== w || state.canvas.height !== h) {
      state.canvas.width = w;
      state.canvas.height = h;
    }
    state.ctx.drawImage(v, 0, srcY, v.videoWidth, srcH, 0, 0, w, h);
    return state.ctx.getImageData(0, 0, w, h);
  }

  function accept(isbn) {
    if (isbn === state.lastText) {
      state.lastCount += 1;
    } else {
      state.lastText = isbn;
      state.lastCount = 1;
    }
    return state.lastCount >= CONFIRMATIONS_REQUIRED;
  }

  function tick() {
    if (!state.running) return;
    var frame = null;
    try {
      frame = grabFrame();
    } catch (e) {
      /* drawImage throws while the track is still warming up */
    }
    if (!frame) return schedule();

    window.ZXingWASM.readBarcodes(frame, {
      formats: ["EAN-13"],
      tryHarder: true,
      maxNumberOfSymbols: 1,
    })
      .then(function (results) {
        if (!state.running) return;
        state.errors = 0;
        var isbn = results && results.length ? isbnFrom(results[0].text) : null;
        if (isbn && accept(isbn)) {
          stop();
          emit("__omnibusOnScanResult", isbn);
          return;
        }
        schedule();
      })
      .catch(function () {
        // A single failed decode is routine (blurred frame, module still
        // warming). Keep looping rather than tearing the camera down — but
        // don't loop forever on a decoder that never works.
        if (!state.running) return;
        if (++state.errors >= MAX_CONSECUTIVE_ERRORS) {
          stop();
          status("error");
          return;
        }
        schedule();
      });
  }

  function schedule() {
    state.timer = setTimeout(tick, FRAME_INTERVAL_MS);
  }

  /* ── Public surface ────────────────────────────────────────────── */

  function stop() {
    state.running = false;
    if (state.timer) {
      clearTimeout(state.timer);
      state.timer = null;
    }
    if (state.stream) {
      state.stream.getTracks().forEach(function (t) {
        t.stop();
      });
    }
    if (state.video) state.video.srcObject = null;
    state.stream = null;
    state.track = null;
    state.video = null;
    state.lastText = null;
    state.lastCount = 0;
    state.errors = 0;
  }

  async function start(videoElementId, opts) {
    stop();
    var video = document.getElementById(videoElementId);
    if (!video) return status("error");
    if (!window.ZXingWASM) return status("error");

    status("starting");
    // Idempotent: zxing-wasm caches the instantiated module, so a re-entry
    // (scan → manual → scan) reuses it instead of re-fetching 1 MB. Awaited so
    // an init failure (e.g. wasm fetch/compile error) reports immediately
    // instead of surfacing ~2.7s later via the tick() retry counter.
    try {
      await window.ZXingWASM.prepareZXingModule({
        overrides: {
          locateFile: function (path, prefix) {
            return path.endsWith(".wasm") && opts && opts.wasmUrl
              ? opts.wasmUrl
              : prefix + path;
          },
        },
      });
    } catch (e) {
      status(classifyError(e));
      return;
    }

    state.video = video;
    state.canvas = state.canvas || document.createElement("canvas");
    state.ctx =
      state.ctx || state.canvas.getContext("2d", { willReadFrequently: true });

    return openCamera()
      .then(function (stream) {
        state.stream = stream;
        state.track = stream.getVideoTracks()[0] || null;
        video.srcObject = stream;
        // iOS only honours inline autoplay when both attributes are set on the
        // element itself, and `play()` still has to be called from here.
        video.setAttribute("playsinline", "");
        video.muted = true;
        return video.play();
      })
      .then(function () {
        reportTorch(state.track);
        state.running = true;
        status("ready");
        schedule();
      })
      .catch(function (err) {
        stop();
        status(classifyError(err));
      });
  }

  function setTorch(on) {
    if (!state.track || !state.track.applyConstraints) return;
    state.track
      .applyConstraints({ advanced: [{ torch: !!on }] })
      .catch(function () {
        // Some drivers advertise torch then refuse the constraint; the button
        // simply doesn't latch, which is better than an error overlay.
      });
  }

  window.OmnibusScanner = { start: start, stop: stop, setTorch: setTorch };
})();
