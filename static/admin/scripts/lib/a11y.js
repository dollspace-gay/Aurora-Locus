// Accessibility utilities — focus trap, aria-live announcer.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §10. Used by Modal, Drawer,
// CommandPalette, Toast.

(function (global) {
  'use strict';

  // Aria-live announcer. Single hidden region used by all status
  // messages, action completions, etc. Per §10.3.
  let liveRegion = null;
  function ensureLiveRegion() {
    if (liveRegion) return liveRegion;
    liveRegion = document.createElement('div');
    liveRegion.setAttribute('aria-live', 'polite');
    liveRegion.setAttribute('aria-atomic', 'true');
    liveRegion.className = 'visually-hidden';
    liveRegion.id = 'aurora-live-region';
    document.body.appendChild(liveRegion);
    return liveRegion;
  }

  function announce(text, priority) {
    const region = ensureLiveRegion();
    if (priority === 'assertive') {
      region.setAttribute('aria-live', 'assertive');
    } else {
      region.setAttribute('aria-live', 'polite');
    }
    // Clear then set so identical successive announcements re-fire.
    region.textContent = '';
    setTimeout(() => { region.textContent = text || ''; }, 50);
  }

  // Find focusable elements within a container.
  const FOCUSABLE = [
    'a[href]',
    'button:not([disabled])',
    'input:not([disabled]):not([type="hidden"])',
    'select:not([disabled])',
    'textarea:not([disabled])',
    '[tabindex]:not([tabindex="-1"])',
  ].join(',');

  function focusableElements(container) {
    if (!container) return [];
    return Array.from(container.querySelectorAll(FOCUSABLE))
      .filter((el) => el.offsetParent !== null || el.tagName === 'DIALOG');
  }

  // Set up a focus trap on a modal-like container. Returns a release
  // function that removes the trap. The caller is responsible for
  // restoring focus after release.
  function trapFocus(container) {
    if (!container) return () => {};
    const previouslyFocused = document.activeElement;

    function onKeyDown(e) {
      if (e.key !== 'Tab') return;
      const items = focusableElements(container);
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }

    container.addEventListener('keydown', onKeyDown);
    // Move focus to first focusable (or container itself) on activation.
    const items = focusableElements(container);
    if (items.length > 0) items[0].focus();
    else if (container.tabIndex >= 0) container.focus();

    return function release() {
      container.removeEventListener('keydown', onKeyDown);
      if (previouslyFocused && typeof previouslyFocused.focus === 'function') {
        previouslyFocused.focus();
      }
    };
  }

  global.AuroraA11y = {
    announce: announce,
    trapFocus: trapFocus,
    focusableElements: focusableElements,
  };
})(window);
