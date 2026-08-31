/* Omnibus site — panel deck.
 *
 * Thirteen full-viewport panels on one translated track. Wheel, keys and touch
 * all funnel through one gate so a trackpad's inertia can't skip three panels
 * at once. Nothing here is required to read the page: the document ships with
 * `body.flow` set, so with JS off every panel stays in the DOM as an ordinary
 * stacked section. This script only ever opts *into* the deck.
 */
(function () {
  'use strict';

  var site = document.getElementById('site');
  var track = document.getElementById('track');
  var rail = document.querySelector('.rail');
  var cue = document.getElementById('cue');
  var panes = Array.prototype.slice.call(track.querySelectorAll('.pane'));
  var n = panes.length;
  var i = 0;
  var lockUntil = 0;
  var acc = 0;
  var touchY = 0;

  var STEP_MS = 980;
  var WHEEL_THRESHOLD = 34;
  var SWIPE_PX = 42;

  /* The deck only engages where it fits. The page ships with body.flow set, so
     a no-JS visit and a phone both get an ordinary scrolling document; this
     removes that class to opt in, and re-checks on resize. */
  var deckOK = window.matchMedia('(min-width: 861px) and (min-height: 560px)');
  var deck = false;

  function setMode() {
    var want = deckOK.matches;
    if (want === deck) return;
    deck = want;
    document.body.classList.toggle('flow', !deck);
    if (deck) { render(); }
    else { track.style.transform = ''; panes.forEach(function (p) { p.classList.add('on'); }); }
  }

  /* ── direction (dark / sepia) ─────────────────────────────────── */
  var DIRS = { dark: 1, sepia: 1 };
  /* own-property check: localStorage and DOM attrs are user-controlled, and
     bare indexing would accept prototype keys like "__proto__" */
  function has(obj, key) { return Object.prototype.hasOwnProperty.call(obj, key); }
  var stored = null;
  try { stored = localStorage.getItem('omn.site.dir'); } catch (e) { /* private mode */ }
  var dir = stored !== null && has(DIRS, stored) ? stored : 'dark';

  /* Shots with a shots/sepia/ counterpart. Anything absent here keeps its
     dark render in both directions rather than 404ing mid-toggle. */
  var SEPIA_SHOTS = {
    'omnibus-android-library': 1, 'omnibus-book-detail': 1,
    'omnibus-checkin-scan': 1,
    'omnibus-checkin-success': 1, 'omnibus-ios-book-detail': 1,
    'omnibus-library-home': 1, 'omnibus-metadata-edit': 1,
    'omnibus-player': 1, 'omnibus-reader-quote-card': 1,
    'omnibus-settings-library': 1, 'omnibus-shelf-kobo': 1,
    'omnibus-sync-alignment': 1, 'omnibus-sync-continue-hero': 1,
    'omnibus-themes-strip': 1, 'omnibus-wishlist-ios': 1
  };

  function retint(d) {
    Array.prototype.forEach.call(document.querySelectorAll('img.shot'), function (img) {
      var m = (img.getAttribute('src') || '').match(/^shots\/[^/]+\/(.+)\.webp$/);
      if (!m) return;
      var want = d === 'sepia' && has(SEPIA_SHOTS, m[1]) ? 'sepia' : 'dark';
      var src = 'shots/' + want + '/' + m[1] + '.webp';
      if (img.getAttribute('src') !== src) img.setAttribute('src', src);
    });
  }

  function setDir(next) {
    if (!has(DIRS, next)) return;
    dir = next;
    site.className = 'site dir-' + dir;
    retint(dir);
    Array.prototype.forEach.call(document.querySelectorAll('.dirtog button'), function (b) {
      var on = b.getAttribute('data-dir') === dir;
      b.classList.toggle('on', on);
      b.setAttribute('aria-pressed', on ? 'true' : 'false');
    });
    try { localStorage.setItem('omn.site.dir', dir); } catch (e) { /* private mode */ }
  }

  Array.prototype.forEach.call(document.querySelectorAll('.dirtog button'), function (b) {
    b.addEventListener('click', function () { setDir(b.getAttribute('data-dir')); });
  });
  setDir(dir);

  /* ── rail ─────────────────────────────────────────────────────── */
  panes.forEach(function (p, k) {
    var label = p.getAttribute('data-label') || p.id;
    var b = document.createElement('button');
    b.type = 'button';
    b.title = label;
    b.setAttribute('aria-label', label);
    b.innerHTML = '<em></em><i></i>';
    b.querySelector('em').textContent = label;
    b.addEventListener('click', function () {
      if (deck) { go(k); return; }
      // the CSS drops every transition under reduced motion; a smooth scroll
      // here would reintroduce exactly the motion that opts out
      var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      panes[k].scrollIntoView({ behavior: still ? 'auto' : 'smooth' });
    });
    rail.appendChild(b);
  });
  var railBtns = Array.prototype.slice.call(rail.children);

  /* ── movement ─────────────────────────────────────────────────── */
  function go(k) {
    k = Math.max(0, Math.min(n - 1, k));
    if (k === i) return;
    i = k;
    render();
  }

  /* The address bar names the panel you are on, so any panel can be linked
     to. replaceState, not a hash assignment: assigning location.hash pushes a
     history entry per panel, which turns Back into a walk through the deck. */
  var stamped = '';
  function stamp(id) {
    if (id === stamped) return;
    stamped = id;
    if (history.replaceState) history.replaceState(null, '', '#' + id);
  }

  function render() {
    track.style.transform = 'translate3d(0,' + (-i * 100) + 'vh,0)';
    panes.forEach(function (p, k) { p.classList.toggle('on', k === i); });
    railBtns.forEach(function (b, k) { b.classList.toggle('on', k === i); });
    cue.classList.toggle('hide', i !== 0);
    stamp(panes[i].id);
  }

  function gate(d) {
    var now = Date.now();
    if (now < lockUntil) return;
    lockUntil = now + STEP_MS;
    go(i + d);
  }

  window.addEventListener('wheel', function (e) {
    if (!deck) return;
    e.preventDefault();
    if (Date.now() < lockUntil) { acc = 0; return; }
    acc += e.deltaY;
    if (Math.abs(acc) > WHEEL_THRESHOLD) { var d = acc > 0 ? 1 : -1; acc = 0; gate(d); }
  }, { passive: false });

  window.addEventListener('keydown', function (e) {
    if (!deck) { if (e.key === '1') setDir('dark'); if (e.key === '2') setDir('sepia'); return; }
    // never swallow a key the user aimed at the toggle or a link
    var t = e.target;
    if (t && (t.tagName === 'BUTTON' || t.tagName === 'A') && (e.key === ' ' || e.key === 'Enter')) return;
    switch (e.key) {
      case 'ArrowDown': case 'PageDown': case ' ': e.preventDefault(); gate(1); break;
      case 'ArrowUp': case 'PageUp': e.preventDefault(); gate(-1); break;
      case 'Home': e.preventDefault(); go(0); break;
      case 'End': e.preventDefault(); go(n - 1); break;
      case '1': setDir('dark'); break;
      case '2': setDir('sepia'); break;
    }
  });

  window.addEventListener('touchstart', function (e) { touchY = e.touches[0].clientY; }, { passive: true });
  window.addEventListener('touchmove', function (e) { if (deck) e.preventDefault(); }, { passive: false });
  window.addEventListener('touchend', function (e) {
    if (!deck) return;
    var last = e.changedTouches[0];
    if (!last) return;
    var dy = touchY - last.clientY;
    if (Math.abs(dy) > SWIPE_PX) gate(dy > 0 ? 1 : -1);
  }, { passive: true });

  /* deep link — /#listen opens on that panel rather than the hero */
  var hash = (location.hash || '').replace('#', '');
  if (hash) {
    var found = panes.findIndex(function (p) { return p.id === hash; });
    if (found > 0) i = found;
  }

  /* ── flow mode: keep the hash on the section being read ─────────
     The deck stamps the hash from render(); flow mode never calls it, so on a
     phone the URL froze at whatever was loaded and no section could be linked
     to. One observer, watching a band across the middle of the viewport, so
     the section that owns the screen is the one that gets named. */
  if (window.IntersectionObserver) {
    var seen = new IntersectionObserver(function (entries) {
      if (deck) return;
      entries.forEach(function (e) { if (e.isIntersecting) stamp(e.target.id); });
    }, { rootMargin: '-45% 0px -45% 0px' });
    panes.forEach(function (p) { seen.observe(p); });
  }

  if (deckOK.addEventListener) deckOK.addEventListener('change', setMode);
  else if (deckOK.addListener) deckOK.addListener(setMode);
  setMode();
})();
