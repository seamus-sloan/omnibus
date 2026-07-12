// Omnibus journal live editor.
//
// A `contenteditable` surface whose `textContent` always stays exactly equal to
// the markdown source — we never add or remove characters, we only wrap
// constructs in decoration spans (`.cm-mark` markers are kept in the text but
// fully faded except on the caret's `.cm-active` line, Obsidian-style). On
// every input we re-render those decorations and restore the caret by absolute
// character offset (offsets are stable because the text length never changes).
//
// The plain markdown is mirrored into a sibling <textarea> (Dioxus owns it via
// its `value`/`oninput`), so publish / validate / preview keep reading the same
// `body` signal they always did — this is purely an editing-surface upgrade.
//
// Loaded once (idempotent guard) and driven from Dioxus via the eval channel:
// the trailing `await dioxus.recv()` block dispatches enhance / attach /
// command / insert. `enhance` is the entry point called from the textarea's
// `onmounted` — it flips the wrapper's `data-omnibus-enhanced` marker (CSS
// hides the textarea, shows the contenteditable) and then delegates to
// `attach` to wire the live editor.

if (!window.OmnibusJournalEditor) {
  window.OmnibusJournalEditor = (function () {
    const ATTR = "data-omnibus-live";
    const byId = (id) => document.getElementById(id);

    // --- caret as an absolute character offset over textContent --------------
    function caret(root) {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0) return null;
      const r = sel.getRangeAt(0);
      if (!root.contains(r.startContainer)) return null;
      const pre = r.cloneRange();
      pre.selectNodeContents(root);
      pre.setEnd(r.startContainer, r.startOffset);
      const start = pre.toString().length;
      return { start, end: start + r.toString().length };
    }
    // Resolve a character offset to a DOM (node, offset) caret position.
    // Walks `.cm-line` spans so a boundary offset lands *inside* the target
    // line rather than at the end of the previous inline span (which the
    // browser would collapse a bare join-text-node caret to). An empty line
    // has no text node, so we return its span at offset 0 — the caret sits
    // before the placeholder `<br>`, keeping typed text on the right line.
    function locate(root, offset) {
      const lines = root.querySelectorAll(":scope > .cm-line");
      let acc = 0;
      for (const line of lines) {
        const len = line.textContent.length;
        if (offset <= acc + len) {
          const local = offset - acc;
          const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT, null);
          let n, a = 0;
          while ((n = walker.nextNode())) {
            if (local <= a + n.nodeValue.length) return { node: n, offset: local - a };
            a += n.nodeValue.length;
          }
          return { node: line, offset: 0 };
        }
        acc += len + 1; // + the literal "\n" joining this line to the next
      }
      const last = lines[lines.length - 1];
      if (!last) return { node: root, offset: 0 };
      const walker = document.createTreeWalker(last, NodeFilter.SHOW_TEXT, null);
      let n, tail = null;
      while ((n = walker.nextNode())) tail = n;
      return tail ? { node: tail, offset: tail.nodeValue.length } : { node: last, offset: 0 };
    }
    function place(root, start, end) {
      const a = locate(root, start), b = locate(root, end);
      const range = document.createRange();
      range.setStart(a.node, a.offset);
      range.setEnd(b.node, b.offset);
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
    }

    // --- markdown -> decorated HTML (textContent preserved exactly) ----------
    const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const mark = (s) => `<span class="cm-mark">${esc(s)}</span>`;

    // Wrap-pairs, longest-first so `**` wins over `*`. Code is last so its body
    // is escaped, not re-tokenized.
    const PAIRS = [
      ["**", "cm-strong"], ["~~", "cm-strike"], ["||", "cm-spoiler"],
      ["*", "cm-em"], ["_", "cm-em"], ["`", "cm-code"],
    ];

    function inline(line) {
      let out = "", i = 0;
      while (i < line.length) {
        if (line[i] === "[") {
          const m = /^\[([^\]\n]+)\]\(([^)\n]+)\)/.exec(line.slice(i));
          if (m) {
            out += `<span class="cm-link">${mark("[")}${esc(m[1])}${mark("](" + m[2] + ")")}</span>`;
            i += m[0].length;
            continue;
          }
        }
        let hit = false;
        for (const [tok, cls] of PAIRS) {
          if (!line.startsWith(tok, i)) continue;
          const close = line.indexOf(tok, i + tok.length);
          if (close <= i + tok.length - 1) continue;
          const inner = line.slice(i + tok.length, close);
          if (!inner.length) continue;
          const body = cls === "cm-code" ? esc(inner) : inline(inner);
          out += `<span class="${cls}">${mark(tok)}${body}${mark(tok)}</span>`;
          i = close + tok.length;
          hit = true;
          break;
        }
        if (hit) continue;
        out += esc(line[i]);
        i++;
      }
      return out;
    }

    function blockLine(line) {
      let m;
      if ((m = /^(#{1,2})\s/.exec(line))) {
        const cls = m[1].length === 1 ? "cm-h1" : "cm-h2";
        return `<span class="${cls}">${mark(m[0])}${inline(line.slice(m[0].length))}</span>`;
      }
      if ((m = /^>\s/.exec(line))) {
        return `<span class="cm-quote">${mark(m[0])}${inline(line.slice(m[0].length))}</span>`;
      }
      if ((m = /^- \[[ xX]\]\s/.exec(line))) {
        return `<span class="cm-task">${mark(m[0])}${inline(line.slice(m[0].length))}</span>`;
      }
      if ((m = /^(\s*)([-*]|\d+\.)\s/.exec(line))) {
        return `${esc(m[1])}<span class="cm-list">${mark(m[2] + " ")}</span>${inline(line.slice(m[0].length))}`;
      }
      return inline(line);
    }

    // Newlines kept as literal text so textContent === the markdown source.
    // Each line is wrapped in a `.cm-line` span (adds no text) so the
    // active-line pass below can reveal markers per line.
    const render = (md) =>
      md
        .split("\n")
        // Empty lines get a `<br>` placeholder so they have layout height (a
        // bare empty inline span collapses to zero and can't be clicked) and a
        // caret landing spot. It contributes no text, so textContent still
        // equals the markdown source exactly.
        .map((line) => `<span class="cm-line">${blockLine(line) || "<br>"}</span>`)
        .join("\n");

    // --- active-line marker reveal (Obsidian-style) ---------------------------
    // Markers are fully faded by CSS except on the `.cm-active` line. The
    // characters stay in the layout either way, so caret-by-offset math and
    // the mirrored markdown are untouched.
    function markActive(editor) {
      const sel = caret(editor);
      const active =
        sel === null ? -1 : editor.textContent.slice(0, sel.start).split("\n").length - 1;
      editor.querySelectorAll(":scope > .cm-line").forEach((line, i) => {
        line.classList.toggle("cm-active", i === active);
      });
    }

    // Caret moves that don't change the text (arrow keys, clicks, blur) never
    // re-render, so one document-level selectionchange listener drives the
    // active-line class across every attached editor. Disconnected editors
    // (unmounted entry edit forms) are dropped as they're encountered.
    const editors = new Set();
    function trackActiveLine(editor) {
      editors.add(editor);
      if (window.__omnibusJournalSelChange) return;
      window.__omnibusJournalSelChange = true;
      document.addEventListener("selectionchange", () => {
        editors.forEach((ed) => {
          if (!ed.isConnected) {
            editors.delete(ed);
            return;
          }
          markActive(ed);
        });
      });
    }

    function sync(editor) {
      const mirror = byId(editor.getAttribute("data-mirror"));
      if (!mirror) return;
      mirror.value = editor.textContent;
      mirror.dispatchEvent(new Event("input", { bubbles: true }));
    }
    function highlight(editor) {
      if (editor.__composing) return;
      const sel = caret(editor);
      editor.innerHTML = render(editor.textContent);
      if (sel) place(editor, sel.start, sel.end);
      markActive(editor);
    }
    function onInput(editor) {
      highlight(editor);
      sync(editor);
    }

    // Progressive enhancement — flip the wrapper's visibility marker so CSS
    // swaps the plain textarea (SSR + first-hydration paint) for the live
    // contenteditable, then wire the editor. Called from the textarea's
    // `onmounted`, which runs on every WASM mount (initial + remount after
    // Cancel/Save). Idempotent per wrapper.
    function enhance(editorId, mirrorId) {
      const mirror = byId(mirrorId);
      if (!mirror) return;
      const wrap = mirror.parentElement;
      if (wrap && wrap.getAttribute("data-omnibus-enhanced") !== "1") {
        wrap.setAttribute("data-omnibus-enhanced", "1");
      }
      attach(editorId, mirrorId);
    }

    function attach(editorId, mirrorId) {
      const editor = byId(editorId);
      if (!editor || editor.getAttribute(ATTR) === "1") return;
      editor.setAttribute(ATTR, "1");
      editor.setAttribute("data-mirror", mirrorId || "");
      // Seed from the mirror's current markdown (edit mode) before wiring.
      const mirror = byId(mirrorId);
      if (mirror && mirror.value) editor.textContent = mirror.value;
      highlight(editor);
      trackActiveLine(editor);

      editor.addEventListener("compositionstart", () => { editor.__composing = true; });
      editor.addEventListener("compositionend", () => {
        editor.__composing = false;
        onInput(editor);
      });
      editor.addEventListener("input", () => onInput(editor));
      editor.addEventListener("keydown", (e) => {
        // Insert a literal newline. `execCommand("insertText", "\n")` is a
        // no-op in Chromium (insertText silently drops newlines), and the raw
        // contenteditable default would add a <br>/<div> that breaks the
        // textContent === markdown invariant. Splice it through the editor's
        // own model instead so textContent gets the real "\n" and re-renders
        // into a fresh `.cm-line`.
        if (e.key === "Enter") {
          e.preventDefault();
          insert(editor.id, "\n");
        }
      });
      editor.addEventListener("paste", (e) => {
        // Plain text only — keep the source clean markdown, never pasted
        // markup. Route through the model (not `execCommand("insertText")`,
        // which drops embedded newlines) so multi-line pastes keep their line
        // breaks.
        e.preventDefault();
        const t = (e.clipboardData || window.clipboardData).getData("text/plain");
        insert(editor.id, t);
      });
    }

    // Apply an edit to the markdown string + selection, re-highlight, restore.
    function edit(editorId, fn) {
      const editor = byId(editorId);
      if (!editor) return;
      const md = editor.textContent;
      const sel = caret(editor) || { start: md.length, end: md.length };
      const res = fn(md, sel.start, sel.end);
      editor.textContent = res.md;
      highlight(editor);
      place(editor, res.start, res.end);
      sync(editor);
      editor.focus();
    }

    function command(editorId, op, a, b) {
      edit(editorId, (md, s, e) => {
        const selText = md.slice(s, e);
        if (op === "wrap") {
          return {
            md: md.slice(0, s) + a + selText + b + md.slice(e),
            start: s + a.length,
            end: s + a.length + selText.length,
          };
        }
        if (op === "prefix") {
          const ls = md.lastIndexOf("\n", s - 1) + 1;
          const block = md.slice(ls, e);
          const prefixed = block.split("\n").map((l) => a + l).join("\n");
          return { md: md.slice(0, ls) + prefixed + md.slice(e), start: ls, end: ls + prefixed.length };
        }
        if (op === "link") {
          const text = selText || a;
          const piece = "[" + text + "](" + b + ")";
          const cs = s + text.length + 3;
          return { md: md.slice(0, s) + piece + md.slice(e), start: cs, end: cs + b.length };
        }
        // insert: drop `a` at the caret, replacing any selection.
        return { md: md.slice(0, s) + a + md.slice(e), start: s + a.length, end: s + a.length };
      });
    }

    const insert = (editorId, text) => command(editorId, "insert", text, "");

    return { enhance, attach, command, insert };
  })();
}

// Dispatch one action from the Dioxus eval channel.
const __omn = await dioxus.recv();
const __E = window.OmnibusJournalEditor;
if (__E && __omn) {
  if (__omn.action === "enhance") __E.enhance(__omn.editorId, __omn.mirrorId);
  else if (__omn.action === "attach") __E.attach(__omn.editorId, __omn.mirrorId);
  else if (__omn.action === "command") __E.command(__omn.editorId, __omn.op, __omn.a, __omn.b);
  else if (__omn.action === "insert") __E.insert(__omn.editorId, __omn.text);
}
