"""Write the macOS app icon from ../mark.svg. Needs rsvg-convert (librsvg).

    python3 app/assets/macos/icon.py

macOS draws every app icon as a tile of its own shape. The mark is a
sticker, so on macOS it sits on a tile of graphite falling to Live Wire's
near-black, untouched: the white bubble with the black scribble.

- AppIcon.icon is the Icon Composer document macOS 26 and later read
  (through Assets.car, which packaging/macos/icon.sh compiles with actool):
  the tile is its fill, the bubble and the scribble are its layers, and
  macOS gives it its shape, edge and shadow, and its dark, clear and tinted
  looks.
- AppIcon.icns is for older macOS and for builds without Xcode 26: the same
  tile drawn on Apple's 1024 grid (an 824 point body with continuous
  corners, and a drop shadow).
"""
import os
import re
import struct
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
TOP, MIDDLE, BOTTOM = "#454B58", "#1B1E26", "#06070B"  # BOTTOM is Live Wire's --canvas.
MARK = 0.64  # The mark's width, as a share of the tile.
NUDGE = 0.015  # Left, by the mark's width: its tail makes it heavy on the right.
VIEWBOX = 167.91972

source = open(os.path.join(HERE, "..", "mark.svg")).read()
paths = dict(re.findall(r'<path fill="(#[0-9a-f]{6})" d="([^"]+)"', source))
BUBBLE, SCRIBBLE = paths["#ffffff"], paths["#000000"]


def mark(d, fill, size, cx, cy):
    """One of the mark's two shapes, size wide, centred on cx, cy."""
    s = size / VIEWBOX
    x, y = cx - size * NUDGE - size / 2, cy - size / 2
    return (
        f'<g transform="translate({x:.3f},{y:.3f}) scale({s:.6f}) '
        f'translate(0.48554,0) translate(2.4985955,2.5004155)" fill-rule="evenodd">'
        f'<path fill="{fill}" d="{d}"/></g>'
    )


def layer(d, fill):
    size = 1024 * MARK
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">'
        + mark(d, fill, size, 512, 512)
        + "</svg>\n"
    )


def squircle(x, y, w, h, r):
    """Apple's continuous-corner rounded rectangle, as cubic Beziers."""
    def p(px, py):
        return f"{x + px:.3f},{y + py:.3f}"

    a, b, c, d, e, f = 1.528665, 1.08849296, 0.86840694, 0.63149379, 0.07491139, 0.37282383
    g = 0.16905956
    return " ".join([
        f"M{p(a*r, 0)}", f"L{p(w - a*r, 0)}",
        f"C{p(w - b*r, 0)} {p(w - c*r, 0)} {p(w - d*r, e*r)}",
        f"C{p(w - f*r, g*r)} {p(w - g*r, f*r)} {p(w - e*r, d*r)}",
        f"C{p(w, c*r)} {p(w, b*r)} {p(w, a*r)}", f"L{p(w, h - a*r)}",
        f"C{p(w, h - b*r)} {p(w, h - c*r)} {p(w - e*r, h - d*r)}",
        f"C{p(w - g*r, h - f*r)} {p(w - f*r, h - g*r)} {p(w - d*r, h - e*r)}",
        f"C{p(w - c*r, h)} {p(w - b*r, h)} {p(w - a*r, h)}", f"L{p(a*r, h)}",
        f"C{p(b*r, h)} {p(c*r, h)} {p(d*r, h - e*r)}",
        f"C{p(f*r, h - g*r)} {p(g*r, h - f*r)} {p(e*r, h - d*r)}",
        f"C{p(0, h - c*r)} {p(0, h - b*r)} {p(0, h - a*r)}", f"L{p(0, a*r)}",
        f"C{p(0, b*r)} {p(0, c*r)} {p(e*r, d*r)}",
        f"C{p(g*r, f*r)} {p(f*r, g*r)} {p(d*r, e*r)}",
        f"C{p(c*r, 0)} {p(b*r, 0)} {p(a*r, 0)}", "Z",
    ])


def legacy_svg():
    body, margin = 824, 100
    tile = squircle(margin, margin, body, body, body * 0.2237)
    size = body * MARK
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
<defs>
 <linearGradient id="fill" x1="0" y1="{margin}" x2="0" y2="{margin + body}" gradientUnits="userSpaceOnUse">
  <stop offset="0" stop-color="{TOP}"/><stop offset="0.55" stop-color="{MIDDLE}"/><stop offset="1" stop-color="{BOTTOM}"/>
 </linearGradient>
 <linearGradient id="edge" x1="0" y1="{margin}" x2="0" y2="{margin + body}" gradientUnits="userSpaceOnUse">
  <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.22"/><stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0.04"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0.10"/>
 </linearGradient>
 <filter id="tile" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur in="SourceAlpha" stdDeviation="14"/><feOffset dy="12"/><feComponentTransfer><feFuncA type="linear" slope="0.30"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>
 <filter id="mark" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur in="SourceAlpha" stdDeviation="16"/><feOffset dy="14"/><feComponentTransfer><feFuncA type="linear" slope="0.5"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>
 <clipPath id="clip"><path d="{tile}"/></clipPath>
</defs>
<g filter="url(#tile)"><path d="{tile}" fill="url(#fill)"/></g>
<g clip-path="url(#clip)"><g filter="url(#mark)">{mark(BUBBLE, "#ffffff", size, 512, 512)}{mark(SCRIBBLE, "#000000", size, 512, 512)}</g></g>
<path d="{tile}" fill="none" stroke="url(#edge)" stroke-width="3"/>
</svg>
'''


def srgb(hex_colour):
    r, g, b = (int(hex_colour[i:i + 2], 16) / 255 for i in (1, 3, 5))
    return f"srgb:{r:.5f},{g:.5f},{b:.5f},1.00000"


def icon_document():
    doc = os.path.join(HERE, "AppIcon.icon")
    os.makedirs(os.path.join(doc, "Assets"), exist_ok=True)
    open(os.path.join(doc, "Assets", "bubble.svg"), "w").write(layer(BUBBLE, "#ffffff"))
    open(os.path.join(doc, "Assets", "scribble.svg"), "w").write(layer(SCRIBBLE, "#000000"))
    open(os.path.join(doc, "icon.json"), "w").write(f'''{{
  "fill" : {{
    "linear-gradient" : [
      "{srgb(TOP)}",
      "{srgb(BOTTOM)}"
    ],
    "orientation" : {{
      "start" : {{
        "x" : 0.5,
        "y" : 0
      }},
      "stop" : {{
        "x" : 0.5,
        "y" : 1
      }}
    }}
  }},
  "groups" : [
    {{
      "layers" : [
        {{
          "glass" : false,
          "image-name" : "scribble.svg",
          "name" : "Scribble"
        }},
        {{
          "glass" : false,
          "image-name" : "bubble.svg",
          "name" : "Bubble"
        }}
      ],
      "shadow" : {{
        "kind" : "neutral",
        "opacity" : 0.5
      }},
      "specular" : true,
      "translucency" : {{
        "enabled" : false,
        "value" : 0
      }}
    }}
  ],
  "supported-platforms" : {{
    "squares" : [
      "macOS"
    ]
  }}
}}
''')


# The PNG sizes an .icns holds, by type: 16 to 512 points, at 1x and 2x.
ICNS = [("icp4", 16), ("icp5", 32), ("ic11", 32), ("ic12", 64), ("ic07", 128),
        ("ic13", 256), ("ic08", 256), ("ic14", 512), ("ic09", 512), ("ic10", 1024)]


def icns():
    with tempfile.TemporaryDirectory() as tmp:
        svg = os.path.join(tmp, "icon.svg")
        open(svg, "w").write(legacy_svg())
        entries = b""
        for kind, px in ICNS:
            png = os.path.join(tmp, f"{px}.png")
            if not os.path.exists(png):
                subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), svg, "-o", png], check=True)
            data = open(png, "rb").read()
            entries += kind.encode() + struct.pack(">I", len(data) + 8) + data
    open(os.path.join(HERE, "AppIcon.icns"), "wb").write(b"icns" + struct.pack(">I", len(entries) + 8) + entries)


icon_document()
icns()
