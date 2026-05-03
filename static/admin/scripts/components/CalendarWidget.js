// CalendarWidget substrate primitive (substrate primitive 20) per
// docs/AURORA_ADMIN_UI_DESIGN.md §6.20.
//
// Locale-aware date and date-range picker. Used by FilterStrip's
// dateRange chip and (future) by forensic-export modal.
//
// Layout: preset chips [Today, 7d, 30d, 90d, Custom] above a
// single-month grid; below grid: text inputs for explicit start/end.
//
// Keyboard: arrow keys navigate dates, Enter selects, Tab moves to
// text inputs. Locale-aware first-day-of-week via Intl.Locale.weekInfo.

(function (global) {
  'use strict';

  const PRESETS = [
    { id: 'today',  label: 'Today', days: 0 },
    { id: 'd7',     label: 'Last 7d', days: 7 },
    { id: 'd30',    label: 'Last 30d', days: 30 },
    { id: 'd90',    label: 'Last 90d', days: 90 },
    { id: 'custom', label: 'Custom', days: null },
  ];

  function startOfDay(d) {
    const r = new Date(d);
    r.setHours(0, 0, 0, 0);
    return r;
  }

  function addDays(d, n) {
    const r = new Date(d);
    r.setDate(r.getDate() + n);
    return r;
  }

  function isoDate(d) {
    if (!(d instanceof Date)) d = new Date(d);
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    return y + '-' + m + '-' + day;
  }

  function buildMonthCells(viewMonth, fdw, range) {
    // viewMonth: Date for first of the month
    const year = viewMonth.getFullYear();
    const month = viewMonth.getMonth();
    const first = new Date(year, month, 1);
    const lastDay = new Date(year, month + 1, 0).getDate();
    const startOffset = (first.getDay() - fdw + 7) % 7;
    const cells = [];
    for (let i = 0; i < startOffset; i++) cells.push(null);
    for (let d = 1; d <= lastDay; d++) cells.push(new Date(year, month, d));
    while (cells.length % 7 !== 0) cells.push(null);
    return cells;
  }

  function inRange(d, range) {
    if (!range || !range.start || !range.end) return false;
    const t = startOfDay(d).getTime();
    return t >= startOfDay(range.start).getTime() && t <= startOfDay(range.end).getTime();
  }

  function build(spec) {
    spec = spec || {};
    const container = spec.container;
    if (!container) return null;
    const fmt = global.AuroraFormat;
    const fdw = fmt ? fmt.firstDayOfWeek() : 0;
    const dayNames = (() => {
      const base = new Date(2026, 5, 7); // Sunday
      const out = [];
      for (let i = 0; i < 7; i++) {
        const dd = addDays(base, (i + fdw) % 7);
        out.push(dd.toLocaleDateString(undefined, { weekday: 'narrow' }));
      }
      return out;
    })();

    let range = spec.initialRange ? Object.assign({}, spec.initialRange) : { start: null, end: null };
    let pickStage = 'start';
    let viewMonth = new Date();
    if (range.start) viewMonth = new Date(range.start.getFullYear(), range.start.getMonth(), 1);
    else viewMonth = new Date(viewMonth.getFullYear(), viewMonth.getMonth(), 1);

    function selectPreset(p) {
      const today = startOfDay(new Date());
      if (p.id === 'today') range = { start: today, end: today };
      else if (p.days != null) range = { start: addDays(today, -p.days), end: today };
      // 'custom' leaves range untouched
      pickStage = 'start';
      render();
    }

    function clickCell(d) {
      if (pickStage === 'start' || !range.start) {
        range = { start: d, end: null };
        pickStage = 'end';
      } else {
        if (d < range.start) range = { start: d, end: range.start };
        else range.end = d;
        pickStage = 'start';
      }
      render();
    }

    function render() {
      let html = '<div class="calendar-widget" role="dialog" aria-label="Calendar">';
      // Presets
      html += '<div class="calendar-presets">';
      const today = startOfDay(new Date());
      for (const p of PRESETS) {
        let pressed = false;
        if (range.start && range.end) {
          if (p.id === 'today' && +range.start === +today && +range.end === +today) pressed = true;
          else if (p.days != null && +range.end === +today && +range.start === +addDays(today, -p.days)) pressed = true;
        }
        html += '<button type="button" class="calendar-preset" data-preset="' + p.id + '"' +
                ' aria-pressed="' + (pressed ? 'true' : 'false') + '">' + p.label + '</button>';
      }
      html += '</div>';
      // Month nav
      const monthLabel = viewMonth.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
      html += '<div class="calendar-nav">';
      html += '  <button type="button" class="calendar-prev" aria-label="Previous month">' +
              (global.AuroraIcons ? global.AuroraIcons.render('chevron-left', 16) : '<') + '</button>';
      html += '  <div>' + monthLabel + '</div>';
      html += '  <button type="button" class="calendar-next" aria-label="Next month">' +
              (global.AuroraIcons ? global.AuroraIcons.render('chevron-right', 16) : '>') + '</button>';
      html += '</div>';
      // Grid
      html += '<div class="calendar-grid" role="grid">';
      for (const dn of dayNames) html += '<div class="calendar-day-name">' + dn + '</div>';
      const cells = buildMonthCells(viewMonth, fdw, range);
      for (const c of cells) {
        if (!c) {
          html += '<div class="calendar-cell empty"></div>';
        } else {
          const t = c.getTime();
          const selected = (range.start && t === startOfDay(range.start).getTime()) ||
                           (range.end && t === startOfDay(range.end).getTime());
          const inR = inRange(c, range);
          html += '<button type="button" class="calendar-cell' + (inR ? ' in-range' : '') + '"' +
                  ' aria-selected="' + (selected ? 'true' : 'false') + '"' +
                  ' aria-label="' + c.toLocaleDateString() + '"' +
                  ' data-iso="' + isoDate(c) + '">' + c.getDate() + '</button>';
        }
      }
      html += '</div>';
      // Text inputs
      html += '<div class="calendar-text-inputs">';
      html += '<label>Start <input type="text" class="calendar-start" value="' + (range.start ? isoDate(range.start) : '') + '" placeholder="YYYY-MM-DD"></label>';
      html += '<label>End <input type="text" class="calendar-end" value="' + (range.end ? isoDate(range.end) : '') + '" placeholder="YYYY-MM-DD"></label>';
      html += '</div>';
      // Footer
      html += '<div style="margin-top: 0.5rem; display: flex; justify-content: flex-end; gap: 0.5rem;">';
      html += '  <button type="button" class="btn-secondary btn-sm calendar-cancel">Cancel</button>';
      html += '  <button type="button" class="btn-primary btn-sm calendar-apply">Apply</button>';
      html += '</div>';
      html += '</div>';
      container.innerHTML = html;
      wire();
    }

    function wire() {
      container.querySelectorAll('.calendar-preset').forEach((el) => {
        el.addEventListener('click', () => {
          const p = PRESETS.find((x) => x.id === el.dataset.preset);
          if (p) selectPreset(p);
        });
      });
      const prev = container.querySelector('.calendar-prev');
      const next = container.querySelector('.calendar-next');
      if (prev) prev.addEventListener('click', () => { viewMonth = new Date(viewMonth.getFullYear(), viewMonth.getMonth() - 1, 1); render(); });
      if (next) next.addEventListener('click', () => { viewMonth = new Date(viewMonth.getFullYear(), viewMonth.getMonth() + 1, 1); render(); });
      container.querySelectorAll('.calendar-cell[data-iso]').forEach((el) => {
        el.addEventListener('click', () => {
          const [y, m, d] = el.dataset.iso.split('-').map(Number);
          clickCell(new Date(y, m - 1, d));
        });
      });
      const sIn = container.querySelector('.calendar-start');
      const eIn = container.querySelector('.calendar-end');
      if (sIn) sIn.addEventListener('change', () => {
        const v = parseDate(sIn.value); if (v) { range.start = v; render(); }
      });
      if (eIn) eIn.addEventListener('change', () => {
        const v = parseDate(eIn.value); if (v) { range.end = v; render(); }
      });
      const cancel = container.querySelector('.calendar-cancel');
      const apply = container.querySelector('.calendar-apply');
      if (cancel) cancel.addEventListener('click', () => { if (typeof spec.onCancel === 'function') spec.onCancel(); });
      if (apply) apply.addEventListener('click', () => {
        if (typeof spec.onApply === 'function') spec.onApply(range);
      });
      // Keyboard nav
      container.addEventListener('keydown', (e) => {
        if (!['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Enter'].includes(e.key)) return;
        const focused = document.activeElement;
        if (!focused || !focused.dataset.iso) return;
        e.preventDefault();
        const [y, m, d] = focused.dataset.iso.split('-').map(Number);
        let target = new Date(y, m - 1, d);
        if (e.key === 'ArrowLeft') target.setDate(target.getDate() - 1);
        else if (e.key === 'ArrowRight') target.setDate(target.getDate() + 1);
        else if (e.key === 'ArrowUp') target.setDate(target.getDate() - 7);
        else if (e.key === 'ArrowDown') target.setDate(target.getDate() + 7);
        else if (e.key === 'Enter') { clickCell(target); return; }
        if (target.getMonth() !== viewMonth.getMonth() || target.getFullYear() !== viewMonth.getFullYear()) {
          viewMonth = new Date(target.getFullYear(), target.getMonth(), 1);
        }
        render();
        const newCell = container.querySelector('.calendar-cell[data-iso="' + isoDate(target) + '"]');
        if (newCell) newCell.focus();
      });
    }

    function parseDate(s) {
      if (!s) return null;
      const m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(s.trim());
      if (!m) return null;
      const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
      return isNaN(d.getTime()) ? null : d;
    }

    render();
    return { getRange: () => range, setRange: (r) => { range = r || { start: null, end: null }; render(); } };
  }

  // Open the calendar inside a modal anchored loosely (modal-style)
  // since FilterStrip doesn't manage popover positioning.
  function openPopover(spec) {
    spec = spec || {};
    const div = document.createElement('div');
    const handle = global.AuroraModal && global.AuroraModal.open({
      title: 'Date range',
      body: div,
    });
    build({
      container: div,
      initialRange: spec.initialRange,
      onApply: (range) => {
        if (typeof spec.onApply === 'function') spec.onApply(range);
        if (handle) handle.close();
      },
      onCancel: () => { if (handle) handle.close(); },
    });
  }

  global.AuroraCalendar = {
    build: build,
    openPopover: openPopover,
  };
})(window);
