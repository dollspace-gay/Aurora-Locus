// Pride theme — random rainbow button hover (#325).
//
// The theme substrate serves CSS only, so this small globally-loaded shim
// provides the one behavior CSS cannot: a *random* 6-stripe rainbow color per
// hover. It is a no-op unless Pride is the active theme (gated on
// <html data-theme="pride">), and the paired CSS in pride/effects.css only
// consumes the variables it sets under [data-theme="pride"] — so it never
// affects any other theme even if it sets a stray inline property.
//
// On hover it picks a random {fill, text} pair (text is white or black,
// whichever clears WCAG AA on that fill — verified by
// components/__tests__/pride-hover-contrast.test.js) and writes them to the
// button's --pride-hover-fill / --pride-hover-text custom properties; on leave
// it clears them. Each hover re-rolls, so moving button-to-button shows a fresh
// color each time (intentional).

(function () {
  'use strict';

  // Standard 6-stripe rainbow, each paired with the AA-clearing text color.
  var COLORS = [
    { fill: '#E40303', text: '#FFFFFF' }, // red
    { fill: '#FF8C00', text: '#000000' }, // orange
    { fill: '#FFED00', text: '#000000' }, // yellow → dark text
    { fill: '#008026', text: '#FFFFFF' }, // green
    { fill: '#004CFF', text: '#FFFFFF' }, // blue
    { fill: '#732982', text: '#FFFFFF' }, // purple
  ];

  // The styled action buttons that carry the rainbow-border treatment.
  var SELECTOR = '.btn-primary, .btn-secondary, .btn-danger, .btn-success';

  function prideActive() {
    return document.documentElement.getAttribute('data-theme') === 'pride';
  }

  function matchedButton(target) {
    return target && target.matches && target.matches(SELECTOR) ? target : null;
  }

  document.addEventListener('mouseenter', function (e) {
    if (!prideActive()) return;
    var btn = matchedButton(e.target);
    if (!btn) return;
    var pick = COLORS[Math.floor(Math.random() * COLORS.length)];
    btn.style.setProperty('--pride-hover-fill', pick.fill);
    btn.style.setProperty('--pride-hover-text', pick.text);
  }, true);

  document.addEventListener('mouseleave', function (e) {
    var btn = matchedButton(e.target);
    if (!btn) return;
    btn.style.removeProperty('--pride-hover-fill');
    btn.style.removeProperty('--pride-hover-text');
  }, true);
})();
