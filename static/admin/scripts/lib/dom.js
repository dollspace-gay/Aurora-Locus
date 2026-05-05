// Small DOM utilities. Vanilla JS, no framework.
//
// Per docs/AURORA_ADMIN_UI_DESIGN.md §12.2: scripts/lib/dom.js holds
// DOM helpers used across components and pages. Keep this small —
// anything page-specific belongs in the page module.

(function (global) {
  'use strict';

  // Escape a value for safe interpolation into HTML. Handles undefined,
  // null, numbers, strings; everything else gets toString'd. Returns
  // an HTML-safe string.
  function esc(value) {
    if (value == null) return '';
    return String(value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  // Build an element from a tag spec, attributes, and children. Useful
  // when string templating gets gnarly. children may be strings (text)
  // or Node objects.
  function el(tag, attrs, children) {
    const node = document.createElement(tag);
    if (attrs) {
      for (const k of Object.keys(attrs)) {
        if (k === 'class' || k === 'className') {
          node.className = attrs[k];
        } else if (k === 'dataset' && attrs[k]) {
          for (const dk of Object.keys(attrs[k])) {
            node.dataset[dk] = attrs[k][dk];
          }
        } else if (k.startsWith('on') && typeof attrs[k] === 'function') {
          node.addEventListener(k.slice(2).toLowerCase(), attrs[k]);
        } else if (attrs[k] !== undefined && attrs[k] !== null && attrs[k] !== false) {
          node.setAttribute(k, attrs[k] === true ? '' : attrs[k]);
        }
      }
    }
    if (Array.isArray(children)) {
      for (const c of children) appendChild(node, c);
    } else if (children != null) {
      appendChild(node, children);
    }
    return node;
  }

  function appendChild(parent, child) {
    if (child == null || child === false) return;
    if (typeof child === 'string' || typeof child === 'number') {
      parent.appendChild(document.createTextNode(String(child)));
    } else if (child instanceof Node) {
      parent.appendChild(child);
    } else if (Array.isArray(child)) {
      for (const c of child) appendChild(parent, c);
    }
  }

  function clear(container) {
    while (container && container.firstChild) container.removeChild(container.firstChild);
  }

  // Mount HTML string into a container, replacing existing content.
  function mount(container, htmlOrNode) {
    if (!container) return;
    clear(container);
    if (htmlOrNode == null) return;
    if (typeof htmlOrNode === 'string') {
      container.innerHTML = htmlOrNode;
    } else if (htmlOrNode instanceof Node) {
      container.appendChild(htmlOrNode);
    }
  }

  // Delegated event helper. Bind once on a parent; matches via selector.
  function delegate(parent, eventName, selector, handler) {
    if (!parent) return;
    parent.addEventListener(eventName, (e) => {
      const target = e.target.closest(selector);
      if (target && parent.contains(target)) handler(e, target);
    });
  }

  global.AuroraDom = {
    esc: esc,
    el: el,
    clear: clear,
    mount: mount,
    delegate: delegate,
  };
})(window);
