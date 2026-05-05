// FilterStrip substrate primitive (substrate primitive 5) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.5.
//
// Horizontal strip of filter controls (text inputs, selects,
// optionally date-range chip backed by CalendarWidget). Pages declare
// the filter shape; component handles rendering + change events.
//
// Spec:
//   {
//     container: HTMLElement,
//     filters: [
//       { type: 'text', id: 'actor', placeholder: 'Filter by actor DID' },
//       { type: 'select', id: 'status', options: [{value, label}], placeholder: 'All' },
//       { type: 'checkbox', id: 'verified', label: 'Verified only' },
//       { type: 'dateRange', id: 'when', label: 'Date range' },  // calendar
//     ],
//     onApply: (values) => void,
//     applyLabel?: 'Apply',
//   }
//
// Returns { getValues, setValues, render }.

(function (global) {
  'use strict';

  function esc(s) { return global.AuroraDom ? global.AuroraDom.esc(s) : String(s == null ? '' : s); }

  function build(spec) {
    if (!spec || !spec.container) return null;
    const c = spec.container;
    const filters = spec.filters || [];
    const initial = spec.initial || {};
    let dateRangeState = {};

    function fieldHtml(f) {
      const id = 'fs-' + f.id;
      const v = initial[f.id];
      switch (f.type) {
        case 'text':
          return '<input type="text" id="' + id + '" placeholder="' + esc(f.placeholder || '') +
                 '" value="' + esc(v || '') + '">';
        case 'select': {
          let opts = '';
          (f.options || []).forEach((o) => {
            const sel = (v != null && v === o.value) ? ' selected' : '';
            opts += '<option value="' + esc(o.value) + '"' + sel + '>' + esc(o.label) + '</option>';
          });
          return '<select id="' + id + '" aria-label="' + esc(f.label || f.placeholder || '') + '">' + opts + '</select>';
        }
        case 'checkbox':
          return '<label style="display:inline-flex; align-items:center; gap:4px;">' +
                 '<input type="checkbox" id="' + id + '"' + (v ? ' checked' : '') + '>' +
                 esc(f.label || '') + '</label>';
        case 'dateRange':
          return '<button type="button" id="' + id + '" class="btn-secondary btn-sm">' +
                 (global.AuroraIcons ? global.AuroraIcons.render('calendar', 14) : '') +
                 ' <span class="dr-label">' + esc(f.label || 'Date range') + '</span></button>';
        default: return '';
      }
    }

    function getValues() {
      const out = {};
      for (const f of filters) {
        const id = 'fs-' + f.id;
        const el = c.querySelector('#' + id);
        if (!el) continue;
        if (f.type === 'checkbox') out[f.id] = el.checked;
        else if (f.type === 'dateRange') out[f.id] = dateRangeState[f.id] || null;
        else out[f.id] = el.value;
      }
      return out;
    }

    function setValues(values) {
      for (const f of filters) {
        const id = 'fs-' + f.id;
        const el = c.querySelector('#' + id);
        if (!el) continue;
        if (f.type === 'checkbox') el.checked = !!values[f.id];
        else if (f.type !== 'dateRange') el.value = values[f.id] != null ? values[f.id] : '';
      }
    }

    function render() {
      let html = '<div class="filter-bar" role="search">';
      for (const f of filters) html += fieldHtml(f);
      html += '<button type="button" class="btn-primary fs-apply">' + esc(spec.applyLabel || 'Apply') + '</button>';
      if (spec.showClear !== false) {
        html += '<button type="button" class="btn-secondary fs-clear">Clear</button>';
      }
      html += '</div>';
      c.innerHTML = html;
      c.querySelector('.fs-apply').addEventListener('click', () => {
        if (typeof spec.onApply === 'function') spec.onApply(getValues());
      });
      const clear = c.querySelector('.fs-clear');
      if (clear) {
        clear.addEventListener('click', () => {
          setValues({});
          dateRangeState = {};
          if (typeof spec.onApply === 'function') spec.onApply(getValues());
        });
      }
      // Wire dateRange controls to CalendarWidget when present.
      for (const f of filters) {
        if (f.type !== 'dateRange') continue;
        const btn = c.querySelector('#fs-' + f.id);
        if (!btn || !global.AuroraCalendar) continue;
        btn.addEventListener('click', () => {
          global.AuroraCalendar.openPopover({
            anchor: btn,
            initialRange: dateRangeState[f.id],
            onApply: (range) => {
              dateRangeState[f.id] = range;
              const lbl = btn.querySelector('.dr-label');
              if (lbl && range && range.start && range.end) {
                const fmt = global.AuroraFormat;
                lbl.textContent = (fmt ? fmt.date(range.start, 'short') : range.start) +
                                  ' – ' + (fmt ? fmt.date(range.end, 'short') : range.end);
              }
              if (typeof spec.onApply === 'function') spec.onApply(getValues());
            },
          });
        });
      }
    }

    render();
    return { getValues: getValues, setValues: setValues, render: render };
  }

  global.AuroraFilterStrip = { build: build };
})(window);
