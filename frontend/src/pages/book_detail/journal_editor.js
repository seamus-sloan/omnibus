// Omnibus journal live editor.
//
// A `contenteditable` surface whose `textContent` always stays exactly equal to
// the markdown source — we never add or remove characters, we only wrap
// constructs in decoration spans (markers kept but dimmed via `.cm-mark`). On
// every input we re-render those decorations and restore the caret by absolute
// character offset (offsets are stable because the text length never changes).
//
// The plain markdown is mirrored into a sibling <textarea> (Dioxus owns it via
// its `value`/`oninput`), so publish / validate / preview keep reading the same
// `body` signal they always did — this is purely an editing-surface upgrade.
//
// Loaded once (idempotent guard) and driven from Dioxus via the eval channel:
// the trailing `await dioxus.recv()` block dispatches attach / command / insert.

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
    function locate(root, offset) {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
      let n, acc = 0;
      while ((n = walker.nextNode())) {
        const len = n.nodeValue.length;
        if (offset <= acc + len) return { node: n, offset: offset - acc };
        acc += len;
      }
      let last = null;
      const w2 = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
      while ((n = w2.nextNode())) last = n;
      return last ? { node: last, offset: last.nodeValue.length } : { node: root, offset: 0 };
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
    const render = (md) => md.split("\n").map(blockLine).join("\n");

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
    }
    function onInput(editor) {
      highlight(editor);
      sync(editor);
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

      editor.addEventListener("compositionstart", () => { editor.__composing = true; });
      editor.addEventListener("compositionend", () => {
        editor.__composing = false;
        onInput(editor);
      });
      editor.addEventListener("input", () => onInput(editor));
      editor.addEventListener("keydown", (e) => {
        // Insert a literal newline (contenteditable would otherwise add a <br>
        // or <div>, breaking the textContent === markdown invariant).
        if (e.key === "Enter") {
          e.preventDefault();
          document.execCommand("insertText", false, "\n");
        }
      });
      editor.addEventListener("paste", (e) => {
        // Plain text only — keep the source clean markdown, never pasted markup.
        e.preventDefault();
        const t = (e.clipboardData || window.clipboardData).getData("text/plain");
        document.execCommand("insertText", false, t);
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

    return { attach, command, insert };
  })();
}

// Dispatch one action from the Dioxus eval channel.
const __omn = await dioxus.recv();
const __E = window.OmnibusJournalEditor;
if (__E && __omn) {
  if (__omn.action === "attach") __E.attach(__omn.editorId, __omn.mirrorId);
  else if (__omn.action === "command") __E.command(__omn.editorId, __omn.op, __omn.a, __omn.b);
  else if (__omn.action === "insert") __E.insert(__omn.editorId, __omn.text);
}
