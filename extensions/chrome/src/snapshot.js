((opts) => {
  const w = window;
  w.__piUidNext = w.__piUidNext || 1;
  const interactive = 'a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=tab],' +
    '[role=menuitem],[role=checkbox],[role=radio],[role=switch],[role=option],[role=combobox],' +
    '[role=textbox],[onclick],[contenteditable=""],[contenteditable=true],[tabindex]:not([tabindex="-1"])';
  const forms = 'input:not([type=hidden]),select,textarea,[contenteditable=""],[contenteditable=true],button[type=submit]';
  const map = 'h1,h2,h3,h4,h5,h6,header,nav,main,aside,footer,form,dialog,[role=dialog],[role=navigation],[role=main]';
  const selector = opts.mode === 'forms' ? forms : opts.mode === 'pageMap' ? map : interactive;
  const clean = (s) => String(s || '').trim().replace(/\s+/g, ' ').slice(0, 120);
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) !== 0;
  };
  const nameOf = (el) => clean(el.getAttribute('aria-label') || (el.labels && el.labels[0] && el.labels[0].innerText) ||
    el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('alt') || el.innerText ||
    (el.tagName === 'INPUT' && /^(submit|button)$/.test(el.type) ? el.value : ''));
  const roleOf = (el) => {
    const role = el.getAttribute('role');
    if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'textarea' || el.isContentEditable) return 'textbox';
    if (tag === 'select') return 'combobox';
    if (tag === 'input') {
      if (/^(checkbox|radio)$/.test(el.type)) return el.type;
      if (/^(submit|button|reset|image)$/.test(el.type)) return 'button';
      return 'textbox';
    }
    return tag;
  };
  const nodes = [...document.querySelectorAll(selector)].filter(visible);
  let elements = nodes.map((el) => {
    let uid = el.getAttribute('data-pi-uid');
    if (!uid) {
      uid = 'e' + w.__piUidNext++;
      el.setAttribute('data-pi-uid', uid);
    }
    const r = el.getBoundingClientRect();
    const e = { uid, role: roleOf(el), name: nameOf(el), x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) };
    if ('value' in el && typeof el.value === 'string' && e.role !== 'button' && el.type !== 'password') e.value = clean(el.value);
    if (el.disabled) e.disabled = true;
    if (el.checked) e.checked = true;
    if (r.bottom < 0 || r.top > innerHeight) e.offscreen = true;
    return e;
  });
  const lower = (s) => String(s || '').toLowerCase();
  if (opts.containingText) elements = elements.filter((e) => lower(e.name + ' ' + (e.value || '')).includes(lower(opts.containingText)));
  if (opts.roleFilter) elements = elements.filter((e) => lower(e.role) === lower(opts.roleFilter));
  if (opts.query) {
    const words = lower(opts.query).split(/\s+/).filter(Boolean);
    const score = (e) => words.filter((word) => lower(e.name + ' ' + e.role + ' ' + (e.value || '')).includes(word)).length;
    elements = elements.map((e) => [score(e), e]).filter(([s]) => s > 0).sort((a, b) => b[0] - a[0]).map(([, e]) => e);
  }
  if (opts.nearUid) {
    const anchor = document.querySelector('[data-pi-uid="' + CSS.escape(opts.nearUid) + '"]');
    if (anchor) {
      const r = anchor.getBoundingClientRect();
      const [ax, ay] = [r.x + r.width / 2, r.y + r.height / 2];
      elements.sort((a, b) => Math.hypot(a.x - ax, a.y - ay) - Math.hypot(b.x - ax, b.y - ay));
    }
  }
  const total = elements.length;
  elements = elements.slice(0, opts.maxElements);
  const text = opts.maxTextChars > 0 ? String(document.body ? document.body.innerText : '').slice(0, opts.maxTextChars) : '';
  return {
    url: location.href, title: document.title, total, elements, text,
    scroll: { y: Math.round(scrollY), height: document.documentElement.scrollHeight, viewport: innerHeight },
  };
})
