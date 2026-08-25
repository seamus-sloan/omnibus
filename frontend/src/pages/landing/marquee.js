// Marquee glue for the library home: raises `lmq--past` once the stack has
// scrolled away (which reveals the edge resume ribbon and retires the scroll
// hint), and drives the shelves row's paging arrows from its scroll position.
// Pure presentation — no app state. Installed from a post-mount effect
// (rule 07: SSR/WASM markup identical); re-running it replaces the previous
// listeners instead of stacking them.
(function () {
  var root = document.getElementById('lmq-root');
  if (!root) return;

  if (window.__omnibusLibMarquee && window.__omnibusLibMarquee.teardown) {
    window.__omnibusLibMarquee.teardown();
  }

  // Past this much scroll the stack is off-screen, so resume has to be
  // reachable some other way. Matches `.lmq--past` in atrium.css.
  var PAST = 240;
  var reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  function paintScroll() {
    root.classList.toggle('lmq--past', window.scrollY > PAST);
  }

  // ── Shelves row paging ────────────────────────────────────────────
  var row = document.getElementById('lmq-shelf-row');
  var left = root.querySelector('.lmq-shnav--l');
  var right = root.querySelector('.lmq-shnav--r');

  function arm(btn, on) {
    if (!btn) return;
    btn.classList.toggle('on', on);
    btn.tabIndex = on ? 0 : -1;
  }

  function measureRow() {
    if (!row) return;
    // The 4px slack absorbs sub-pixel scroll widths, which would otherwise
    // leave the right arrow permanently lit on a row that has no overflow.
    arm(left, row.scrollLeft > 4);
    arm(right, row.scrollLeft + row.clientWidth < row.scrollWidth - 4);
  }

  function nudge(dir) {
    if (!row) return;
    row.scrollBy({
      left: dir * Math.max(240, row.clientWidth * 0.6),
      behavior: reduced ? 'auto' : 'smooth',
    });
  }

  var onLeft = function () { nudge(-1); };
  var onRight = function () { nudge(1); };
  if (left) left.addEventListener('click', onLeft);
  if (right) right.addEventListener('click', onRight);
  if (row) row.addEventListener('scroll', measureRow, { passive: true });

  // The shelf list arrives after mount, so the row's scrollWidth changes
  // under us — observe it rather than measuring once.
  var ro = null;
  if (row && typeof ResizeObserver !== 'undefined') {
    ro = new ResizeObserver(measureRow);
    ro.observe(row);
    for (var i = 0; i < row.children.length; i++) ro.observe(row.children[i]);
  }
  var mo = null;
  if (row && typeof MutationObserver !== 'undefined') {
    mo = new MutationObserver(function () {
      measureRow();
      if (!ro) return;
      for (var j = 0; j < row.children.length; j++) ro.observe(row.children[j]);
    });
    mo.observe(row, { childList: true });
  }

  function onResize() {
    measureRow();
    paintScroll();
  }
  window.addEventListener('scroll', paintScroll, { passive: true });
  window.addEventListener('resize', onResize);

  window.__omnibusLibMarquee = {
    teardown: function () {
      window.removeEventListener('scroll', paintScroll);
      window.removeEventListener('resize', onResize);
      if (row) row.removeEventListener('scroll', measureRow);
      if (left) left.removeEventListener('click', onLeft);
      if (right) right.removeEventListener('click', onRight);
      if (ro) ro.disconnect();
      if (mo) mo.disconnect();
      window.__omnibusLibMarquee = null;
    },
  };

  paintScroll();
  measureRow();
})();
