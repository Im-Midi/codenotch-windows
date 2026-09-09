/* Shared presentation rules. No credentials or network access belong in this file. */
(function (root) {
  const model = {
    headline(snap, choice = 'highest') {
      const windows = (snap.windows || []).filter(w => w.count != null || Number.isFinite(w.used));
      const selected = choice === 'weekly' ? windows.find(w => /weekly/i.test(w.label)) : windows.find(w => w.id === choice);
      if (selected) return selected;
      const metered = windows.filter(w => w.count == null);
      return metered.length ? metered.reduce((a, b) => b.used > a.used ? b : a) : windows[0] || null;
    },
    stale(snap, now = Date.now()) {
      return snap.status !== 'ok' || !snap.fetched_at || now - snap.fetched_at > 300000 ||
        (snap.windows || []).some(w => w.resets_at && w.resets_at <= now);
    },
    percent(used) { return Math.round(Math.min(1, Math.max(0, used)) * 100); },
    clamp(value, min, max) { return Math.max(min, Math.min(value, Math.max(min, max))); },
    escape(value) { return String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])); }
  };
  if (typeof module !== 'undefined') module.exports = model;
  else root.NotchModel = model;
})(globalThis);
