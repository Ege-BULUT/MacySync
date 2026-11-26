"""Writes docs/logo.svg and site/logo.svg: the MYSY ANSI Shadow art drawn as shapes, so it does not depend on fonts.

The same generator as WavySync's logo, in MacySync's metallic blue and steel."""
M = ["███╗   ███╗", "████╗ ████║", "██╔████╔██║", "██║╚██╔╝██║", "██║ ╚═╝ ██║", "╚═╝     ╚═╝"]
Y = ["██╗   ██╗", "╚██╗ ██╔╝", " ╚████╔╝ ", "  ╚██╔╝  ", "   ██║   ", "   ╚═╝   "]
S = ["███████╗", "██╔════╝", "███████╗", "╚════██║", "███████║", "╚══════╝"]
rows = ["".join(g[r] for g in (M, Y, S, Y)) for r in range(6)]
CW, CH = 16, 22
W, H = 1000, 320
ncol = len(rows[0])
X0 = (W - ncol * CW) / 2
Y0 = 48
# which half-segments each box glyph has: l r u d
SEG = {"═": "lr", "║": "ud", "╗": "ld", "╔": "rd", "╚": "ur", "╝": "ul"}


def lines(ch, x, y):
    cx, cy = x + CW / 2, y + CH / 2
    out = []
    for off in (-2.5, 2.5):
        for s in SEG[ch]:
            if s == "l": out.append(f"M{x:g} {cy+off:g}H{cx+off*(1 if ch in '╗╝' else -1):g}")
            if s == "r": out.append(f"M{cx-off*(1 if ch in '╔╚' else -1):g} {cy+off:g}H{x+CW:g}")
            if s == "u": out.append(f"M{cx+off:g} {y:g}V{cy-off*(1 if ch in '╚╝' else -1):g}")
            if s == "d": out.append(f"M{cx+off:g} {cy+off*(1 if ch in '╗╔' else -1):g}V{y+CH:g}")
    return "".join(out)


cols = []
for c in range(ncol):
    blocks, shadow = [], []
    for r, row in enumerate(rows):
        ch, x, y = row[c], X0 + c * CW, Y0 + r * CH
        if ch == "█":
            blocks.append(f'<rect x="{x:g}" y="{y:g}" width="{CW+0.6:g}" height="{CH+0.6:g}"/>')
        elif ch in SEG:
            shadow.append(lines(ch, x, y))
    if not blocks and not shadow:
        continue
    b = "".join(blocks)
    s = f'<path class="sh" d="{"".join(shadow)}"/>' if shadow else ""
    cols.append(f'<g class="c" style="animation-delay:-{(ncol-c)*0.08:.2f}s">{s}<g fill="url(#g)">{b}</g><g fill="url(#sheen)">{b}</g></g>')

swift_a = "M 30 262 C 250 262 330 222 500 216 S 760 176 970 172"
swift_b = "M 60 276 C 280 276 350 236 500 230 S 740 192 950 190"
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="MacySync">
<defs>
<linearGradient id="g" x1="0" y1="{Y0}" x2="0" y2="{Y0+6*CH}" gradientUnits="userSpaceOnUse">
<stop offset="0" stop-color="#dfe8f3"/><stop offset=".3" stop-color="#8cc4ff"/><stop offset=".62" stop-color="#4c9dff"/><stop offset="1" stop-color="#1b3f8a"/></linearGradient>
<linearGradient id="sheen" x1="-260" y1="0" x2="-60" y2="0" gradientUnits="userSpaceOnUse">
<stop offset="0" stop-color="#fff" stop-opacity="0"/><stop offset=".5" stop-color="#fff" stop-opacity=".5"/><stop offset="1" stop-color="#fff" stop-opacity="0"/>
<animateTransform attributeName="gradientTransform" type="translate" values="0 0;{W+320} 0" dur="5s" repeatCount="indefinite"/></linearGradient>
<linearGradient id="line" x1="0" x2="1"><stop offset="0" stop-color="#2563c9" stop-opacity="0"/><stop offset=".45" stop-color="#4c9dff"/><stop offset="1" stop-color="#c3ccd8" stop-opacity="0"/></linearGradient>
</defs>
<style>
.c {{ animation: bob 2.8s ease-in-out infinite; }}
@keyframes bob {{ 0%, 100% {{ transform: translateY(0); }} 50% {{ transform: translateY(-7px); }} }}
.sh {{ fill: none; stroke: #7d8a9c; stroke-width: 1.3; stroke-linecap: square; opacity: .85; }}
.sw {{ fill: none; stroke-linecap: round; }}
.base {{ stroke: #2c3a4f; stroke-width: 1.2; }}
.a {{ stroke: url(#line); stroke-width: 3; stroke-dasharray: 240 1300; animation: flow 3.4s linear infinite; }}
.b {{ stroke: #a8d4ff; stroke-width: 1.5; stroke-dasharray: 90 1000; animation: flow 4.6s linear infinite reverse; opacity: .7; }}
@keyframes flow {{ from {{ stroke-dashoffset: 1400; }} to {{ stroke-dashoffset: 0; }} }}
.t {{ font: 600 15px -apple-system, "Segoe UI", Helvetica, Arial, sans-serif; fill: #c3ccd8; letter-spacing: 3px; }}
@media (prefers-reduced-motion: reduce) {{ .c, .a, .b {{ animation: none; }} }}
</style>
<rect width="{W}" height="{H}" rx="26" fill="#0e131b"/>
<rect x=".5" y=".5" width="{W-1}" height="{H-1}" rx="25.5" fill="none" stroke="#2c3a4f"/>
<path class="sw base" d="{swift_a}"/>
<path class="sw a" d="{swift_a}"/>
<path class="sw b" d="{swift_b}"/>
{"".join(cols)}
<text class="t" x="{W/2:g}" y="{H-26}" text-anchor="middle">YOUR MAC <tspan fill="#4c9dff">~</tspan> ANY MAC <tspan fill="#4c9dff">~</tspan> END TO END</text>
</svg>
'''
for path in ("docs/logo.svg", "site/logo.svg"):
    open(path, "w").write(svg)
print(len(svg), "bytes")
