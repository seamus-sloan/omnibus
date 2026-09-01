// Flow scroll glue for the book-detail stage — the marquee unrolled (Option
// B, the default). Same job as `marquee.js` and the same teardown contract,
// but the panel is one continuous scroller instead of six snapped screens:
// the cover drifts with the scroll rather than panning stop to stop, the dot
// rail jumps to a section's offset instead of a screen index, the active dot
// follows whichever section the reading line has passed, and a "back to the
// book" pill appears once the top is out of sight. Pure presentation — no app
// state. Installed from a post-mount effect (rule 07: SSR/WASM markup
// identical); re-running it replaces the previous listeners rather than
// stacking them.
(function () {
  var scroller = document.getElementById('bdmq-flow');
  if (!scroller) return;
  var cover = document.getElementById('bdmq-coverpx');
  var dots = Array.prototype.slice.call(document.querySelectorAll('.bdmq-dotrow'));
  var secs = Array.prototype.slice.call(document.querySelectorAll('.bdmq-flowsec'));
  var top = document.getElementById('bdmq-flowtop');
  var reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // The marquee and the flow are alternative installs on the same stage, so
  // they share one teardown slot — flipping the setting must not leave the
  // other mode's listeners bound to a container it no longer owns.
  if (window.__omnibusMarquee && window.__omnibusMarquee.teardown) window.__omnibusMarquee.teardown();

  function measureTopbar() {
    var bar = document.querySelector('.atrium-topbar');
    if (bar) {
      document.documentElement.style.setProperty('--bdmq-top', bar.offsetHeight + 'px');
    }
  }

  // How far the section list has run past the reading line, as a 0..1 share
  // of the scrollable distance. Guarded against a zero range: a short book
  // whose flow fits the viewport has nothing to drift over.
  function progress() {
    var range = scroller.scrollHeight - scroller.clientHeight;
    return range > 0 ? scroller.scrollTop / range : 0;
  }

  function paint() {
    if (cover) {
      var over = Math.max(0, cover.offsetHeight - scroller.clientHeight);
      // Reduced motion keeps the cover centred rather than drifting.
      var y = reduced ? over / 2 : over / 2 - progress() * over;
      cover.style.transform = 'translateY(' + y + 'px)';
    }
    // The active section is the last one whose top has passed the reading
    // line — a fixed offset below the scroller's top edge, so a heading that
    // has only just appeared doesn't claim the rail before it is being read.
    var line = scroller.scrollTop + 120;
    var at = 0;
    for (var i = 0; i < secs.length; i++) {
      if (secs[i].offsetTop <= line) at = i;
    }
    for (var j = 0; j < dots.length; j++) dots[j].classList.toggle('on', j === at);
    if (top) top.classList.toggle('on', scroller.scrollTop > 400);
  }

  function goTo(i) {
    var el = secs[i];
    if (!el) return;
    // Section 0 is the book itself and carries no label, so it scrolls to a
    // true zero rather than to its own offset minus the label's gap.
    scroller.scrollTo({
      top: i === 0 ? 0 : Math.max(0, el.offsetTop - 30),
      behavior: reduced ? 'auto' : 'smooth',
    });
  }

  function onResize() {
    measureTopbar();
    paint();
  }

  var dotHandlers = dots.map(function (d, i) {
    var h = function () {
      goTo(i);
    };
    d.addEventListener('click', h);
    return h;
  });
  var topHandler = function () {
    goTo(0);
  };
  if (top) top.addEventListener('click', topHandler);
  scroller.addEventListener('scroll', paint, { passive: true });
  window.addEventListener('resize', onResize);

  window.__omnibusMarquee = {
    teardown: function () {
      scroller.removeEventListener('scroll', paint);
      window.removeEventListener('resize', onResize);
      if (top) top.removeEventListener('click', topHandler);
      for (var i = 0; i < dots.length; i++) dots[i].removeEventListener('click', dotHandlers[i]);
      window.__omnibusMarquee = null;
    },
  };

  measureTopbar();
  // A fresh book always opens at the top — without this, navigating from one
  // book's journal section to another book keeps the old scroll offset.
  scroller.scrollTo({ top: 0, behavior: 'auto' });
  paint();
})();
