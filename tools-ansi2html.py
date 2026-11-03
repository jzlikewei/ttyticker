import re, html, sys

data = open('/tmp/pt-frame.txt', 'rb').read().decode('utf-8')
lines = data.split('\r\n')
CELL = re.compile(r'\x1b\[38;2;(\d+);(\d+);(\d+)m\x1b\[48;2;(\d+);(\d+);(\d+)m(.)', re.S)

rows = []
for line in lines:
    if not line.strip():
        continue
    spans = []
    buf = []
    cf = cb = None
    pos = 0
    for m in CELL.finditer(line):
        fg = (int(m.group(1)), int(m.group(2)), int(m.group(3)))
        bg = (int(m.group(4)), int(m.group(5)), int(m.group(6)))
        ch = m.group(7)
        if fg == cf and bg == cb:
            buf.append(ch)
        else:
            if buf:
                spans.append((cf, cb, ''.join(buf)))
            buf = [ch]
            cf, cb = fg, bg
    if buf:
        spans.append((cf, cb, ''.join(buf)))
    inner = ''.join(
        '<span style="color:rgb(%d,%d,%d);background:rgb(%d,%d,%d)">%s</span>' % (
            f[0], f[1], f[2], b[0], b[1], b[2], html.escape(t).replace(' ', '&nbsp;'))
        for f, b, t in spans)
    rows.append('<div class="r">%s</div>' % inner)

doc = """<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<title>PerfTicker 实机渲染帧</title>
<style>
  body { margin:0; background:#0b0e14; padding:24px; }
  .frame { background:#131722; padding:14px 16px; border-radius:10px;
           border:1px solid #2b2b43; display:inline-block;
           box-shadow:0 12px 40px rgba(0,0,0,.55); }
  .r { font: 13px/1.18 "SF Mono", Menlo, "DejaVu Sans Mono", monospace;
       white-space: pre; letter-spacing: 0; }
</style></head>
<body><main><div class="frame">
""" + '\n'.join(rows) + """
</div></main></body></html>"""

open('/tmp/pt-frame.html', 'w').write(doc)
print('rows', len(rows))
