// A small Liquid + HTML highlighter for the playground. It only colors text: every character of
// the source comes out once, in order, so the result lines up with the textarea above it.

const KEYWORDS = new Set([
  'and', 'or', 'contains', 'in', 'with', 'as', 'for', 'by', 'limit', 'offset', 'reversed',
  'true', 'false', 'nil', 'null', 'empty', 'blank', 'cols',
]);
const RAW_BODY = new Set(['raw', 'comment', 'schema', 'javascript', 'stylesheet', 'style', 'doc']);

const escape = (s) => s.replace(/[&<>]/g, (c) => (c === '&' ? '&amp;' : c === '<' ? '&lt;' : '&gt;'));
const span = (cls, text) => (text ? `<span class="tk-${cls}">${escape(text)}</span>` : '');

// The inside of {{ }} or {% %}. `tagName` says whether the first word names a tag.
function liquidInner(src, tagName) {
  let out = '';
  let i = 0;
  let expectName = tagName;
  let expectFilter = false;
  while (i < src.length) {
    const rest = src.slice(i);
    let m;
    if ((m = /^\s+/.exec(rest))) {
      // In {% liquid %}, each line starts with a tag name.
      if (tagName === 'lines' && m[0].includes('\n')) expectName = true;
      out += escape(m[0]);
    } else if ((m = /^#[^\n]*/.exec(rest)) && expectName) {
      out += span('comment', m[0]);
      expectName = false;
    } else if ((m = /^("[^"]*"?|'[^']*'?)/.exec(rest))) {
      out += span('str', m[0]);
      expectName = false;
    } else if ((m = /^-?\d+(\.\d+)?/.exec(rest))) {
      out += span('num', m[0]);
      expectName = false;
    } else if ((m = /^[A-Za-z_][\w-]*\??/.exec(rest))) {
      const word = m[0];
      if (expectName) out += span('tag', word);
      else if (expectFilter) out += span('filter', word);
      else if (KEYWORDS.has(word)) out += span('kw', word);
      else if (/^\s*:/.test(src.slice(i + word.length))) out += span('arg', word);
      else out += span('var', word);
      expectName = false;
      expectFilter = false;
    } else if ((m = /^(==|!=|<>|<=|>=|<|>|=|\.\.)/.exec(rest))) {
      out += span('op', m[0]);
    } else if (rest[0] === '|') {
      m = ['|'];
      out += span('op', '|');
      expectFilter = true;
    } else {
      m = [rest[0]];
      out += span('punct', rest[0]);
    }
    i += m[0].length;
  }
  return out;
}

// Finds the end of a Liquid tag or output that starts at `i`, closing delimiter included.
function liquidEnd(src, i, close) {
  const end = src.indexOf(close, i + 2);
  return end === -1 ? src.length : end + 2;
}

// One {{ }} or {% %} at `i`. Returns [html, nextIndex].
function liquid(src, i) {
  const isTag = src[i + 1] === '%';
  const end = liquidEnd(src, i, isTag ? '%}' : '}}');
  const text = src.slice(i, end);
  const closed = text.endsWith(isTag ? '%}' : '}}') && text.length >= 4;
  const open = /^\{[{%]-?/.exec(text)[0];
  const close = closed ? /-?[}%]\}$/.exec(text)[0] : '';
  const inner = text.slice(open.length, text.length - close.length);
  const name = isTag ? (/^\s*([A-Za-z_]\w*|#)/.exec(inner) || [])[1] : null;

  if (name === '#') return [span('comment', text), end];

  let html = span('delim', open) + liquidInner(inner, name === 'liquid' ? 'lines' : isTag) + span('delim', close);

  // Tags whose bodies aren't Liquid: highlight up to the matching end tag as one block.
  if (name && RAW_BODY.has(name)) {
    const endTag = new RegExp(`\\{%-?\\s*end${name}\\s*-?%\\}`, 'g');
    endTag.lastIndex = end;
    const m = endTag.exec(src);
    const bodyEnd = m ? m.index : src.length;
    const body = src.slice(end, bodyEnd);
    html += name === 'comment' || name === 'doc' ? span('comment', body) : escape(body);
    if (m) html += liquid(src, m.index)[0];
    return [html, m ? m.index + m[0].length : src.length];
  }
  return [html, end];
}

// Text that may hold Liquid but no HTML, up to `stop` (exclusive).
function textWithLiquid(src, i, stop, cls) {
  let out = '';
  while (i < stop) {
    const next = src.slice(i, stop).search(/\{[{%]/);
    const until = next === -1 ? stop : i + next;
    if (until > i) out += cls ? span(cls, src.slice(i, until)) : escape(src.slice(i, until));
    if (until >= stop) break;
    const [html, end] = liquid(src, until);
    out += html;
    i = Math.min(end, src.length);
    if (end > stop) return [out, end];
  }
  return [out, stop];
}

// An HTML tag at `i`. Returns [html, nextIndex, tagName, isClosing].
function htmlTag(src, i) {
  const m = /^<(\/?)([A-Za-z][\w:-]*)/.exec(src.slice(i));
  let out = span('punct', '<' + m[1]) + span('el', m[2]);
  i += m[0].length;
  while (i < src.length) {
    const rest = src.slice(i);
    let t;
    if ((t = /^\/?>/.exec(rest))) {
      out += span('punct', t[0]);
      return [out, i + t[0].length, m[2].toLowerCase(), !!m[1]];
    } else if (/^\{[{%]/.test(rest)) {
      const [html, end] = liquid(src, i);
      out += html;
      i = end;
      continue;
    } else if ((t = /^\s+/.exec(rest))) {
      out += escape(t[0]);
    } else if (rest[0] === '"' || rest[0] === "'") {
      const q = rest[0];
      const close = src.indexOf(q, i + 1);
      const stop = close === -1 ? src.length : close + 1;
      out += span('str', q);
      const [html, end] = textWithLiquid(src, i + 1, close === -1 ? stop : close, 'str');
      out += html;
      if (end > stop) { i = end; continue; }
      if (close !== -1) out += span('str', q);
      i = stop;
      continue;
    } else if ((t = /^[^\s"'<>={}\/]+/.exec(rest))) {
      out += span('attr', t[0]);
    } else {
      t = [rest[0]];
      out += span('punct', rest[0]);
    }
    i += t[0].length;
  }
  return [out, i, m[2].toLowerCase(), !!m[1]];
}

export function highlight(src) {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const rest = src.slice(i);
    if (/^\{[{%]/.test(rest)) {
      const [html, end] = liquid(src, i);
      out += html;
      i = end;
    } else if (rest.startsWith('<!--')) {
      const close = src.indexOf('-->', i + 4);
      const end = close === -1 ? src.length : close + 3;
      out += span('comment', src.slice(i, end));
      i = end;
    } else if (/^<\/?[A-Za-z]/.test(rest)) {
      const [html, end, name, closing] = htmlTag(src, i);
      out += html;
      i = end;
      // Script and style bodies aren't HTML: only Liquid inside them is highlighted.
      if (!closing && (name === 'script' || name === 'style')) {
        const close = src.slice(i).search(new RegExp(`</${name}\\b`, 'i'));
        const stop = close === -1 ? src.length : i + close;
        const [body, next] = textWithLiquid(src, i, stop);
        out += body;
        i = next;
      }
    } else {
      const next = rest.search(/\{[{%]|<(\/?[A-Za-z]|!--)/);
      const end = next <= 0 ? (next === 0 ? i + 1 : src.length) : i + next;
      out += escape(src.slice(i, end));
      i = end;
    }
  }
  return out;
}

// Wavy underlines for offenses: the source as invisible text, with ranges wrapped. Offsets are
// UTF-16 indexes, like JavaScript strings. Overlapping ranges keep the first.
export function marks(src, offenses) {
  const ranges = offenses
    .map((o) => [o.start.index, Math.max(o.end.index, o.start.index + 1), o.severity])
    .sort((a, b) => a[0] - b[0]);
  let out = '';
  let pos = 0;
  for (const [start, end, sev] of ranges) {
    if (start < pos || start >= src.length) continue;
    const stop = Math.min(end, src.length);
    out += escape(src.slice(pos, start)) + `<span class="sq ${sev}">${escape(src.slice(start, stop))}</span>`;
    pos = stop;
  }
  return out + escape(src.slice(pos));
}
