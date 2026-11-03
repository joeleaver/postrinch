#!/usr/bin/env python3
"""Stand-in for an app-side pipeline: spec parse -> sanitize -> legacy attrs to
CSS -> <table> to CSS grid. Writes out/normalized/<fixture>.html.

html5lib stands in for html5ever (no network for new crates here). The table
rewrite is deliberately cheap: it is the 'can we avoid a table formatting
context' experiment, not a table layout algorithm.
"""
import os, re, sys

# --parsed: parse and sanitize only, for a rinch that lays tables out itself.
PARSED = '--parsed' in sys.argv
OUT = 'out/parsed' if PARSED else 'out/normalized'
import html5lib

DROP = {'script', 'iframe', 'object', 'embed', 'form', 'input', 'button', 'select',
        'textarea', 'meta', 'title', 'link', 'base', 'style', 'head', 'svg', 'math'}
VOID = {'br', 'hr', 'img'}
NON_FLOW = {'tr', 'tbody', 'thead', 'tfoot', 'caption', 'colgroup', 'col'}
SIZES = {'1': '10px', '2': '13px', '3': '16px', '4': '18px', '5': '24px', '6': '32px', '7': '48px'}


def is_el(n):
    return isinstance(n.tag, str)


def sty(n, css, front=True):
    cur = n.get('style', '').strip().rstrip(';')
    n.set('style', f'{css};{cur}' if front and cur else (f'{cur};{css}' if cur else css))


def dim(v):
    v = (v or '').strip()
    m = re.match(r'^(\d+(?:\.\d+)?)(%|px)?', v)
    return None if not m else m.group(1) + (m.group(2) or 'px')


def clean(n):
    for c in list(n):
        if is_el(c) and '}' in c.tag:  # foreign content (svg/math) has a namespaced tag
            c.tag = 'svg'
        if not is_el(c):  # comment: conditional-comment payloads die here
            tail = c.tail
            n.remove(c)
            if tail:
                n.text = (n.text or '') + tail
        elif c.tag in DROP:
            n.remove(c)
        else:
            clean(c)
    for k in list(n.keys()):
        if k.lower().startswith('on'):
            del n.attrib[k]
    if n.get('href', '').strip().lower().startswith('javascript:'):
        del n.attrib['href']
    if re.match(r'https?:', n.get('src', '')):  # remote images blocked by default
        n.attrib.pop('src')
    if 'style' in n.attrib:
        n.set('style', scrub_css(n.get('style')))


def scrub_css(css):
    css = re.sub(r'position\s*:\s*(fixed|sticky)', 'position:static', css, flags=re.I)
    css = re.sub(r'@import[^;]*;', '', css, flags=re.I)
    css = re.sub(r'url\(\s*["\']?https?:[^)]*\)', 'none', css, flags=re.I)
    return re.sub(r'mso-[a-z-]+\s*:[^;}]*;?', '', css, flags=re.I)


def attrs(n):
    t = n.tag
    if n.get('bgcolor'):
        sty(n, f'background-color:{n.get("bgcolor")}')
    if t == 'font':
        n.tag = 'span'
        if n.get('color'): sty(n, f'color:{n.get("color")}')
        if n.get('face'): sty(n, f'font-family:{n.get("face")}')
        if n.get('size') in SIZES: sty(n, f'font-size:{SIZES[n.get("size")]}')
    if t == 'center':
        n.tag = 'div'
        sty(n, 'text-align:center')
        for c in n:
            if is_el(c) and c.tag == 'table' and not c.get('align'):
                c.set('align', 'center')
    if t in ('p', 'div', 'h1', 'h2', 'h3', 'td', 'th') and n.get('align'):
        sty(n, f'text-align:{n.get("align").lower()}')
    if t == 'img':
        if n.get('hspace'): sty(n, f'margin-left:{n.get("hspace")}px;margin-right:{n.get("hspace")}px')
        if n.get('align', '').lower() in ('left', 'right'): sty(n, f'float:{n.get("align").lower()}')
    for c in n:
        attrs(c)
    if t == 'table':
        table(n)


def table(t):
    pad = t.get('cellpadding')
    collapse = 'collapse' in t.get('style', '')
    spacing = t.get('cellspacing', '0' if collapse else '2')
    border = t.get('border', '0') not in ('0', '')
    grid, cols, rows = {}, [], []
    for s in t:
        if not is_el(s): continue
        rows += [s] if s.tag == 'tr' else [r for r in s if is_el(r) and r.tag == 'tr']
    cells = []
    for y, r in enumerate(rows):
        x = 0
        for c in [c for c in r if is_el(c) and c.tag in ('td', 'th')]:
            while (x, y) in grid: x += 1
            cs, rs = int(c.get('colspan') or 1), int(c.get('rowspan') or 1)
            for i in range(cs):
                for j in range(rs): grid[(x + i, y + j)] = 1
            w = dim(c.get('width'))
            if cs == 1 and w:
                while len(cols) <= x: cols.append(None)
                cols[x] = cols[x] or w
            cells.append((c, r, x, y, cs, rs))
            x += cs
    width = 1 + max([k[0] for k in grid] or [0])
    cols += [None] * (width - len(cols))
    tracks = ' '.join('auto' if not w else f'{float(w[:-1])}fr' if w.endswith('%') else w for w in cols)
    out = []
    for c, r, x, y, cs, rs in cells:
        rstyle = re.sub(r'(^|;)\s*(display|width|height)[^;]*', '', r.get('style', ''))
        if r.get('bgcolor') and not c.get('bgcolor'): sty(c, f'background-color:{r.get("bgcolor")}')
        if rstyle: sty(c, rstyle)
        va = (c.get('valign') or r.get('valign') or 'middle').lower()
        just = {'top': 'flex-start', 'bottom': 'flex-end'}.get(va, 'center')
        ta = (c.get('align') or r.get('align') or ('center' if c.tag == 'th' else '')).lower()
        base = f'grid-column:{x + 1} / span {cs};grid-row:{y + 1} / span {rs};display:flex;flex-direction:column;justify-content:{just};min-width:0'
        if c.tag == 'th': base += ';font-weight:bold'
        if pad is not None: base += f';padding:{pad}px'
        elif not collapse: base += ';padding:1px'
        if border: base += ';border:1px solid #888'
        base += f';text-align:{ta or "left"}'
        if c.get('height') and dim(c.get('height')): base += f';height:{dim(c.get("height"))}'
        sty(c, base)
        inner = c.makeelement('div', {})
        inner.text, c.text = c.text, None
        for k in list(c):
            c.remove(k); inner.append(k)
        c.append(inner)
    for s in t.iter():
        if s is not t and s.tag in ('tbody', 'thead', 'tfoot', 'tr'):
            sty(s, 'display:contents')
    w = dim(t.get('width'))
    css = f'display:grid;grid-template-columns:{tracks};gap:{spacing}px;box-sizing:border-box'
    if w: css += f';width:{w}'
    if w and t.get('width').strip().rstrip('px').isdigit(): css += ';max-width:100%'
    if t.get('align') == 'center': css += ';margin-left:auto;margin-right:auto'
    if border: css += ';border:1px solid #888'
    sty(t, css)


def esc(s):
    return s.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def ser(n):
    a = ''.join(f' {k}="{esc(v).replace(chr(34), chr(39))}"' for k, v in n.items()
                if PARSED or k in ('style', 'class', 'id', 'href', 'src', 'alt', 'width', 'height', 'dir', 'role'))
    head = f'<{n.tag}{a}>'
    if n.tag in VOID: return head + esc(n.tail or '')
    return head + esc(n.text or '') + ''.join(ser(c) for c in n) + f'</{n.tag}>' + esc(n.tail or '')


UA = '' if PARSED else """table{width:fit-content}
a{color:#0000ee;text-decoration:underline}
li{display:list-item;list-style-type:disc}
"""

os.makedirs(OUT, exist_ok=True)
for f in sorted(os.listdir('fixtures')):
    raw = open('fixtures/' + f, encoding='utf-8').read()
    doc = html5lib.parse(raw, treebuilder='etree', namespaceHTMLElements=False)
    css = '\n'.join((s.text or '').replace('<!--', '').replace('-->', '') for s in doc.iter('style'))
    body = doc.find('body')
    clean(body)
    if not PARSED:
        attrs(body)
        if body.get('text'): sty(body, f'color:{body.get("text")}')
    elif body.get('bgcolor'):
        sty(body, f'background-color:{body.get("bgcolor")}')
    body.tag = 'div'
    body.attrib.pop('width', None)
    bstyle = body.attrib.pop('style', '')
    inner = esc(body.text or '') + ''.join(ser(c) for c in body)
    open(f'{OUT}/' + f, 'w', encoding='utf-8').write(
        f'<html><head><style>{UA}{scrub_css(css)}</style></head><body style="{bstyle.replace(chr(34), chr(39))}">{inner}</body></html>')
print('normalized', len(os.listdir(OUT)))
