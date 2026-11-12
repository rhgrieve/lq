import { highlight, marks } from './highlight.js';

// One example per file type, checked as that path. Each has only problems that Apply fixes
// and Format can clear, so the demo ends clean.
const EXAMPLES = {
  'snippets/product-card.liquid': `{% assign card_class = 'card' %}
<div class="{{card_class}}">
<a href="{{ product.url | within: collection }}">
<img src="{{ product.featured_image | image_url: width: 600 }}" alt="{{ product.title | escape }}" width="600" height="600" loading="lazy">
</a>
<h3>{{product.title|escape}}</h3>
{% if product.compare_at_price > product.price %}<span class="badge">Sale</span>{%endif%}
<p style="color: {{ settings.accent | hex_to_rgba: 0.8 }}">{{ product.price | money }}</p>
<div class="stars">{% for star in (1..4.5) %}<span>★</span>{% endfor %}</div>
<script src="{{ 'card.js' | asset_url }}" defer></script>
<style>.card{display:grid;gap:8px}</style>
</div>
`,
  'sections/featured.liquid': `<section class="featured" style="background: {{ section.settings.background | hex_to_rgba: 0.1 }}">
<h2>{{section.settings.heading|escape}}</h2>
<div class="featured__grid">{% for block in section.blocks %}<div class="featured__item" {{ block.shopify_attributes }}>{{ block.settings.text }}</div>{% endfor %}</div>
</section>
{% schema %}
{"name":"Featured","settings":[{"type":"text","id":"heading","label":"Heading","default":"New arrivals"},{"type":"color","id":"background","label":"Background","default":"#f4f1ea"}],"blocks":[{"type":"text","name":"Text","settings":[{"type":"text","id":"text","label":"Text"}]}],"presets":[{"name":"Featured"}]}
{% endschema %}
`,
  'layout/theme.liquid': `<!doctype html>
<html lang="{{ request.locale.iso_code }}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{{ page_title }}{% if current_page != 1 %} – Page {{ current_page }}{% endif %}</title>
{{ 'theme.css' | asset_url | stylesheet_tag }}
</head>
<body>
<main id="MainContent">{{ content_for_layout }}</main>
</body>
</html>
`,
};
const DEFAULT_PATH = 'snippets/product-card.liquid';

// Bump the version when EXAMPLES change, so returning visitors see the new one.
const STORE_KEY = 'lq-playground-2';
const $ = (id) => document.getElementById(id);
const els = {
  source: $('source'), gutter: $('gutter'), hl: $('hl'), marks: $('marks'), path: $('path'), width: $('width'), tabs: $('tabs'),
  format: $('format'), fix: $('fix'), status: $('status'), timing: $('timing'), offenses: $('offenses'),
  version: $('version'),
};

let worker = null;
let ready = false;
let nextId = 1;
let latestCheck = 0;
let lastResult = null;
let checkTimer = 0;

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE_KEY) || 'null');
    if (saved && typeof saved.source === 'string' && EXAMPLES[saved.path]) {
      els.source.value = saved.source;
      els.path.value = saved.path;
      if (saved.width) els.width.value = saved.width;
      els.tabs.checked = !!saved.tabs;
      return;
    }
  } catch {}
  els.path.value = DEFAULT_PATH;
  els.source.value = EXAMPLES[DEFAULT_PATH];
}

function save() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify({
      source: els.source.value, path: els.path.value, width: els.width.value, tabs: els.tabs.checked,
    }));
  } catch {}
}

function startWorker() {
  ready = false;
  worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
  worker.onmessage = ({ data }) => {
    if (data.type === 'ready') {
      ready = true;
      els.format.disabled = false;
      els.version.textContent = `Playground runs lq ${data.version}.`;
      runCheck();
    } else if (data.type === 'failed') {
      setStatus(`Couldn't load lq: ${data.message}`);
    } else if (data.type === 'dead') {
      startWorker();
    } else if (data.type === 'result') {
      onResult(data);
    }
  };
  worker.onerror = (e) => setStatus(`Couldn't load lq${e.message ? `: ${e.message}` : '.'}`);
}

function send(op) {
  if (!ready) return 0;
  const id = nextId++;
  worker.postMessage({
    id, op, source: els.source.value, path: els.path.value,
    options: { printWidth: Number(els.width.value) || 120, useTabs: els.tabs.checked },
  });
  return id;
}

function runCheck() {
  clearTimeout(checkTimer);
  latestCheck = send('check');
}

function scheduleCheck() {
  clearTimeout(checkTimer);
  checkTimer = setTimeout(runCheck, 200);
}

function onResult({ id, op, result, error, ms }) {
  if (error) {
    setStatus(error);
    els.timing.textContent = '';
    lastResult = null;
    els.fix.disabled = true;
    els.offenses.replaceChildren();
    renderEditor([]);
    return;
  }
  if (op === 'format') {
    if (!result.ok) {
      const at = result.line ? ` (line ${result.line}${result.column ? `, column ${result.column}` : ''})` : '';
      setStatus(`Can't format: ${result.message}${at}`);
      return;
    }
    const changed = result.output !== els.source.value;
    if (changed) replaceSource(result.output);
    els.timing.textContent = `formatted in ${ms.toFixed(1)} ms`;
    if (!changed) setStatus('Already formatted.');
    return;
  }
  if (id !== latestCheck) return;
  if (result.error) {
    setStatus(result.error);
    return;
  }
  lastResult = result;
  renderOffenses(result.offenses);
  els.fix.disabled = !result.fixed;
  els.timing.textContent = `checked in ${ms.toFixed(1)} ms`;
}

function setStatus(text) {
  els.status.textContent = text;
}

function plural(n, word) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function renderOffenses(offenses) {
  const counts = { error: 0, warning: 0, info: 0 };
  for (const o of offenses) counts[o.severity]++;
  if (!offenses.length) {
    setStatus('No problems found.');
  } else {
    const parts = [];
    if (counts.error) parts.push(plural(counts.error, 'error'));
    if (counts.warning) parts.push(plural(counts.warning, 'warning'));
    if (counts.info) parts.push(counts.info + ' info');
    setStatus(parts.join(', '));
  }

  els.offenses.replaceChildren(...offenses.map((o) => {
    const li = document.createElement('li');
    li.className = 'offense';

    const sev = document.createElement('span');
    sev.className = `sev ${o.severity}`;
    sev.textContent = o.severity;

    const loc = document.createElement('button');
    loc.type = 'button';
    loc.className = 'loc';
    loc.textContent = `${o.start.line + 1}:${o.start.character + 1}`;
    loc.setAttribute('aria-label', `Go to line ${o.start.line + 1}, column ${o.start.character + 1}`);
    loc.addEventListener('click', () => select(o));

    const body = document.createElement('div');
    body.className = 'msg';
    const text = document.createElement('span');
    text.textContent = o.message;
    body.append(text);

    const meta = document.createElement('div');
    meta.className = 'meta';
    if (o.url) {
      const a = document.createElement('a');
      a.href = o.url;
      a.textContent = o.check;
      a.target = '_blank';
      a.rel = 'noopener';
      meta.append(a);
    } else {
      const c = document.createElement('code');
      c.textContent = o.check;
      meta.append(c);
    }
    if (o.fixable) {
      const f = document.createElement('span');
      f.className = 'fixable';
      f.textContent = 'auto-fixable';
      meta.append(f);
    }
    for (const s of o.suggestions || []) {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'suggest';
      b.textContent = s.message;
      b.title = 'Apply this suggestion';
      b.addEventListener('click', (e) => {
        e.stopPropagation();
        replaceSource(s.fixed);
      });
      meta.append(b);
    }
    body.append(meta);

    li.append(sev, loc, body);
    li.addEventListener('click', (e) => {
      if (e.target.closest('a, button')) return;
      select(o);
    });
    return li;
  }));
  renderEditor(offenses);
}

function renderEditor(offenses = lastResult ? lastResult.offenses : []) {
  const lines = els.source.value.split('\n').length;
  const worst = new Map();
  const rank = { error: 0, warning: 1, info: 2 };
  for (const o of offenses) {
    const l = o.start.line;
    if (!worst.has(l) || rank[o.severity] < rank[worst.get(l)]) worst.set(l, o.severity);
  }
  const frag = document.createDocumentFragment();
  for (let i = 0; i < lines; i++) {
    const sev = worst.get(i);
    if (sev) {
      const s = document.createElement('span');
      s.className = `hit ${sev}`;
      s.textContent = String(i + 1);
      frag.append(s, '\n');
    } else {
      frag.append(`${i + 1}\n`);
    }
  }
  els.gutter.replaceChildren(frag);

  // The trailing newline keeps the layers as tall as the textarea's content.
  const text = els.source.value;
  els.hl.innerHTML = highlight(text) + '\n ';
  els.marks.innerHTML = marks(text, offenses) + '\n ';
  syncScroll();
}

function syncScroll() {
  const { scrollTop, scrollLeft } = els.source;
  els.gutter.scrollTop = scrollTop;
  for (const layer of [els.hl, els.marks]) {
    layer.scrollTop = scrollTop;
    layer.scrollLeft = scrollLeft;
  }
}

// Offsets from lq are UTF-16 indexes, the same units as the textarea's.
function select(o) {
  const ta = els.source;
  ta.focus();
  const end = Math.max(o.end.index, o.start.index);
  ta.setSelectionRange(o.start.index, end);
  const lineHeight = parseFloat(getComputedStyle(ta).lineHeight) || 20;
  ta.scrollTop = Math.max(0, o.start.line * lineHeight - ta.clientHeight / 3);
}

// Replaces the whole text through the editing pipeline where possible, so undo still works.
function replaceSource(text) {
  const ta = els.source;
  const { scrollTop } = ta;
  ta.focus();
  ta.select();
  let ok = false;
  try { ok = document.execCommand('insertText', false, text); } catch {}
  if (!ok || ta.value !== text) {
    ta.value = text;
    ta.dispatchEvent(new Event('input'));
  }
  ta.setSelectionRange(0, 0);
  ta.scrollTop = scrollTop;
}

els.source.addEventListener('input', () => {
  renderEditor();
  save();
  scheduleCheck();
});
els.source.addEventListener('scroll', syncScroll);
els.source.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault();
    send('format');
    return;
  }
  // Tab indents. Press Escape first to move focus out with Tab instead.
  if (e.key === 'Escape') { els.source.dataset.escaped = '1'; return; }
  if (e.key === 'Tab' && !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey && !els.source.dataset.escaped) {
    e.preventDefault();
    document.execCommand('insertText', false, els.tabs.checked ? '\t' : '  ');
  }
  if (e.key !== 'Escape') delete els.source.dataset.escaped;
});
// Picking an example replaces the editor's text; undo brings the previous text back.
els.path.addEventListener('change', () => replaceSource(EXAMPLES[els.path.value]));
els.width.addEventListener('change', save);
els.tabs.addEventListener('change', save);
els.format.addEventListener('click', () => send('format'));
els.reset = $('reset');
els.reset.addEventListener('click', () => replaceSource(EXAMPLES[els.path.value]));
els.fix.addEventListener('click', () => { if (lastResult && lastResult.fixed) replaceSource(lastResult.fixed); });

const copy = $('copy-prompt');
copy.addEventListener('click', async () => {
  const text = $('prompt-text').textContent;
  try {
    await navigator.clipboard.writeText(text);
    copy.textContent = 'Copied';
  } catch {
    const range = document.createRange();
    range.selectNodeContents($('prompt-text'));
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    copy.textContent = 'Press ⌘C / Ctrl+C';
  }
  setTimeout(() => { copy.textContent = 'Copy'; }, 2000);
});

load();
renderEditor([]);
startWorker();
