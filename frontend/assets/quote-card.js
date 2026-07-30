/*
 * quote-card.js — Omnibus shareable quote-card renderer.
 *
 * Standalone (no epub.js / JSZip dependency): draws the fixed quote-card
 * layout onto an offscreen canvas and exports it as a PNG. Loaded by every
 * page that mounts the quote-card editor — the reader's quote drawer and the
 * book-detail "Passages you saved" modal.
 *
 * Public surface: window.OmnibusQuoteCard
 *   exportQuoteCard(json)           canvas PNG → <a download>
 *   shareQuoteCard(json)            canvas PNG → navigator.share(files),
 *                                   download fallback
 *   copyQuoteCardImage(json)        canvas PNG → clipboard, download fallback
 *
 * `json` is { text, author, subtitle, bg, ink, ratio, filename }.
 *
 * Native-share shim (optional; mobile shell only): when
 * `window.__omnibusOnShareImage` is defined, shareQuoteCard routes here
 * instead of the Web Share API (absent in WKWebView) and the host presents
 * the OS share sheet. The payload is { name, dataUrl } with a base64 PNG
 * data URL.
 */
(function () {
  "use strict";

  // Draw the quote card onto an offscreen canvas — the shared renderer behind
  // the export / share / copy actions. Drawn directly (rather than
  // rasterizing DOM) because the card is a fixed, bespoke layout — a solid
  // background, an italic serif quote, and an attribution footer — so
  // hand-drawing yields crisp output with no heavyweight DOM-capture
  // dependency. Returns null on a bad payload.
  function renderQuoteCanvas(json) {
    var o;
    try {
      o = JSON.parse(json);
    } catch (e) {
      return null;
    }
    var ratios = { "1:1": [1080, 1080], "4:5": [1080, 1350], "9:16": [1080, 1920], "3:4": [1080, 1440] };
    var dim = ratios[o.ratio] || ratios["1:1"];
    var W = dim[0], H = dim[1];
    var canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    var ctx = canvas.getContext("2d");
    if (!ctx) return null;
    var pad = Math.round(W * 0.1);
    ctx.fillStyle = o.bg || "#1a1a1a";
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = o.ink || "#ffffff";
    ctx.textBaseline = "top";

    // Header kicker.
    ctx.font = "600 " + Math.round(W * 0.018) + "px ui-monospace, monospace";
    ctx.globalAlpha = 0.55;
    ctx.fillText("OMNIBUS · QUOTE", pad, pad);
    ctx.globalAlpha = 1;

    // Quote body — word-wrapped italic serif.
    var quote = "“" + (o.text || "") + "”";
    var fontPx = Math.round(W * 0.052);
    ctx.font = "italic " + fontPx + "px Georgia, 'Times New Roman', serif";
    var maxW = W - pad * 2;
    var words = quote.split(/\s+/);
    var lines = [];
    var line = "";
    for (var i = 0; i < words.length; i++) {
      var test = line ? line + " " + words[i] : words[i];
      if (ctx.measureText(test).width > maxW && line) {
        lines.push(line);
        line = words[i];
      } else {
        line = test;
      }
    }
    if (line) lines.push(line);
    var lineH = Math.round(fontPx * 1.3);
    var blockH = lines.length * lineH;
    var startY = Math.max(pad * 2.2, (H - blockH) / 2 - lineH);
    for (var j = 0; j < lines.length; j++) {
      ctx.fillText(lines[j], pad, startY + j * lineH);
    }

    // Attribution footer.
    var footY = H - pad - Math.round(W * 0.06);
    ctx.globalAlpha = 0.85;
    ctx.fillRect(pad, footY - Math.round(W * 0.02), W - pad * 2, 2);
    ctx.font = Math.round(W * 0.022) + "px ui-sans-serif, system-ui, sans-serif";
    ctx.fillText((o.author || "").toUpperCase(), pad, footY);
    ctx.font = "italic " + Math.round(W * 0.026) + "px Georgia, serif";
    ctx.fillText(o.subtitle || "", pad, footY + Math.round(W * 0.035));
    ctx.globalAlpha = 1;
    return { canvas: canvas, name: (o.filename || "omnibus-quote") + ".png", title: o.subtitle || "Quote" };
  }

  function downloadBlob(blob, name) {
    var url = URL.createObjectURL(blob);
    var a = document.createElement("a");
    a.href = url;
    a.download = name;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(function () {
      URL.revokeObjectURL(url);
    }, 1000);
  }

  function exportQuoteCard(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      downloadBlob(blob, r.name);
    }, "image/png");
  }

  // Native share sheet with the rendered PNG. The mobile shell's
  // `__omnibusOnShareImage` shim takes priority (WKWebView has no usable Web
  // Share API — Rust presents the real sheet); after that, Web Share where it
  // can take files, then a plain download (desktop browsers, older WebViews).
  function shareQuoteCard(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    if (typeof window.__omnibusOnShareImage === "function") {
      try {
        window.__omnibusOnShareImage(JSON.stringify({
          name: r.name,
          dataUrl: r.canvas.toDataURL("image/png"),
        }));
      } catch (e) {
        /* ignore handler errors */
      }
      return;
    }
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      try {
        var f = new File([blob], r.name, { type: "image/png" });
        if (navigator.canShare && navigator.canShare({ files: [f] })) {
          navigator.share({ files: [f], title: r.title }).catch(function () {
            /* user dismissed the sheet */
          });
          return;
        }
      } catch (e) {
        /* File/Web Share unsupported */
      }
      downloadBlob(blob, r.name);
    }, "image/png");
  }

  // Copy the rendered PNG to the clipboard; falls back to a download when
  // ClipboardItem is unavailable.
  function copyQuoteCardImage(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      try {
        if (navigator.clipboard && window.ClipboardItem) {
          var item = new ClipboardItem({ "image/png": blob });
          navigator.clipboard.write([item]).catch(function () {
            downloadBlob(blob, r.name);
          });
          return;
        }
      } catch (e) {
        /* ClipboardItem unsupported */
      }
      downloadBlob(blob, r.name);
    }, "image/png");
  }

  window.OmnibusQuoteCard = {
    exportQuoteCard: exportQuoteCard,
    shareQuoteCard: shareQuoteCard,
    copyQuoteCardImage: copyQuoteCardImage,
  };
})();
