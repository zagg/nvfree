#!/usr/bin/env python3
"""tools/capture.py - render a real nvfree session into a PNG.

Runs the binary inside a pty (so it detects a terminal, picks truecolour and
takes the window size), records the bytes, replays them through a small VT
model (printable cells, LF, CUUs-n, EL-0, ED-0, SGR) and draws the last screen
with PIL. Only used for the README figure - nothing is vendored.

    python3 tools/capture.py --out docs/nvfree.png --cols 96 -- --procs
    python3 tools/capture.py --frames 6 --loop 0.4 -- --procs      # live mode
"""
import argparse
import fcntl
import os
import pty
import re
import struct
import subprocess
import sys
import termios

BASIC = {
    30: (17, 17, 17), 31: (222, 86, 86), 32: (108, 201, 110), 33: (225, 199, 86),
    34: (94, 147, 228), 35: (192, 120, 215), 36: (88, 192, 199), 37: (204, 204, 204),
    90: (110, 110, 118), 91: (255, 122, 122), 92: (140, 230, 140), 93: (245, 220, 110),
    94: (120, 170, 255), 95: (215, 145, 240), 96: (120, 220, 230), 97: (245, 245, 245),
}
# 256-colour cube, coarse: only used if nvfree ever falls back to 38;5;n
X256 = [BASIC.get(30 + i % 8, (200, 200, 200)) for i in range(16)]


def spawn(argv, cols, rows, hold=None):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    env = dict(os.environ, COLORTERM="truecolor", TERM="xterm-256color",
               LINES=str(rows), COLUMNS=str(cols))
    p = subprocess.Popen(argv, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
    os.close(slave)
    out = b""
    if hold:
        import time
        time.sleep(hold)
        p.terminate()
    while True:
        try:
            chunk = os.read(master, 65536)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    p.wait()
    os.close(master)
    return out.decode("utf-8", "replace")


class Screen:
    """le sous-ensemble VT strictement necessaire a nvfree"""

    def __init__(self, cols, rows=64):
        self.cols = cols
        self.rows = [[(" ", None, False, False) for _ in range(cols)] for _ in range(rows)]
        self.cy = self.cx = 0

    def grow(self):
        while len(self.rows) <= self.cy:
            self.rows.append([(" ", None, False, False) for _ in range(self.cols)])

    def put(self, ch, fg, bold, dim):
        self.grow()
        if self.cx < self.cols:
            self.rows[self.cy][self.cx] = (ch, fg, bold, dim)
            self.cx += 1

    def newline(self):
        self.grow()
        self.cy += 1
        self.cx = 0

    def erase_line(self):
        self.grow()
        for x in range(self.cx, self.cols):
            self.rows[self.cy][x] = (" ", None, False, False)

    def erase_display(self):
        for x in range(self.cx, self.cols):
            self.grow()
            self.rows[self.cy][x] = (" ", None, False, False)
        for y in range(self.cy + 1, len(self.rows)):
            self.rows[y] = [(" ", None, False, False) for _ in range(self.cols)]

    def up(self, n):
        self.cy = max(0, self.cy - n)

    def text_grid(self):
        last = 0
        for i, row in enumerate(self.rows):
            if any(c[0] != " " for c in row):
                last = i
        return [row for row in self.rows[: last + 1]]


def replay(text, cols):
    sc = Screen(cols)
    fg, bold, dim = None, False, False
    i = 0
    while i < len(text):
        c = text[i]
        if c == "\x1b":
            m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", text[i:])
            if m:
                params, cmd = m.group(1), m.group(2)
                if cmd == "m":
                    parts = [int(x) if x else 0 for x in params.split(";")] if params else [0]
                    j = 0
                    while j < len(parts):
                        p = parts[j]
                        if p == 0:
                            fg, bold, dim = None, False, False
                        elif p == 1:
                            bold = True
                        elif p == 2:
                            dim = True
                        elif p in (21, 22, 23, 24):
                            bold = dim = False
                        elif p == 38 and j + 2 < len(parts) and parts[j + 1] == 2:
                            fg = tuple(parts[j + 2:j + 5])
                            j += 4
                        elif p == 38 and j + 2 < len(parts) and parts[j + 1] == 5:
                            n = parts[j + 2]
                            fg = X256[n % 16] if n < 16 else (128 + 27 * ((n - 16) // 36),
                                                              128 + 27 * (((n - 16) // 6) % 6),
                                                              128 + 27 * ((n - 16) % 6))
                            j += 2
                        elif 30 <= p <= 37 or 90 <= p <= 97:
                            fg = BASIC[p]
                        j += 1
                elif cmd == "A":
                    sc.up(int(params or 1))
                elif cmd == "B":
                    sc.cy += int(params or 1)
                elif cmd == "C":
                    sc.cx += int(params or 1)
                elif cmd == "D":
                    sc.cx = max(0, sc.cx - int(params or 1))
                elif cmd == "G":
                    sc.cx = int(params or 1) - 1
                elif cmd == "K":
                    sc.erase_line()
                elif cmd == "J":
                    sc.erase_display()
                elif cmd == "H":
                    p = [int(x) if x else 1 for x in params.split(";")] if params else [1, 1]
                    sc.cy, sc.cx = (p[0] - 1, p[1] - 1) if len(p) > 1 else (0, 0)
                i += m.end()
                continue
            m = re.match(r"\x1b\][^\x07\x1b]*(\x07|\x1b\\)", text[i:])
            if m:
                i += m.end()
                continue
            i += 1
            continue
        if c == "\r":
            sc.cx = 0
        elif c == "\n":
            sc.newline()
        elif c == "\t":
            sc.cx = (sc.cx // 8 + 1) * 8
        elif c >= " ":
            sc.put(c, fg, bold, dim)
        i += 1
    return sc.text_grid()


def draw(grid, font_path, bold_path, size=16, scale=2, title="nvfree"):
    from PIL import Image, ImageDraw, ImageFont

    f = ImageFont.truetype(font_path, size)
    fb = ImageFont.truetype(bold_path, size)
    cw, ch = int(size * 0.605 * scale), int(size * 1.24 * scale)
    cols = max(len(r) for r in grid) if grid else 1
    pad, bar = 16, 32
    W = cols * cw + pad * 2
    H = len(grid) * ch + pad * 2 + bar
    img = Image.new("RGB", (W, H), (18, 20, 26))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, W, bar], fill=(30, 33, 41))
    tf = ImageFont.truetype(font_path, int(size * 0.85))
    d.text((pad, 9), title, font=tf, fill=(150, 156, 172))
    for x, col in enumerate([(240, 95, 86), (255, 192, 66), (83, 204, 112)]):
        d.ellipse([W - pad - 16 - x * 22, 11, W - pad + 2 - x * 22, 27], fill=col)
    y0 = bar + pad
    for y, row in enumerate(grid):
        for x, cell in enumerate(row):
            glyph, col, b, dm = cell
            if glyph == " ":
                continue
            c = col if col is not None else (214, 216, 223)
            if dm:
                c = tuple(int(v * 0.62) for v in c)
            d.text((pad + x * cw, y0 + y * ch), glyph, font=(fb if b else f), fill=c)
    return img


def main():
    argv = sys.argv[1:]
    sub = []
    if "--" in argv:
        k = argv.index("--")
        sub, argv = argv[k + 1:], argv[:k]
    a = argparse.ArgumentParser(description="capture an nvfree frame as a PNG")
    a.add_argument("--out", default="docs/nvfree.png")
    a.add_argument("--cols", type=int, default=96)
    a.add_argument("--rows", type=int, default=20)
    a.add_argument("--size", type=int, default=16)
    a.add_argument("--scale", type=int, default=2)
    a.add_argument("--title", default="gpucalc - nvfree, VRAM read from the driver")
    a.add_argument("--bin", default=os.environ.get("NVFREE_BIN", "./target/release/nvfree"))
    a.add_argument("--loop", type=float, default=0.0, help="capture the live mode for N seconds")
    ns = a.parse_args(argv)
    if not sub:
        sub = ["--procs"]
    cmd = [ns.bin] + sub
    if ns.loop:
        cmd += ["-l", "0.5", "--width", str(ns.cols)]
        text = spawn(cmd, ns.cols, ns.rows, hold=ns.loop)
    else:
        cmd += ["--width", str(ns.cols)]
        text = spawn(cmd, ns.cols, ns.rows)
    grid = replay(text, ns.cols)
    if not grid:
        print("capture: no output", file=sys.stderr)
        return 1
    os.makedirs(os.path.dirname(ns.out) or ".", exist_ok=True)
    img = draw(grid, "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
               "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", ns.size, ns.scale, ns.title)
    img.save(ns.out)
    print("wrote %s (%dx%d)" % (ns.out, img.width, img.height))
    return 0


if __name__ == "__main__":
    sys.exit(main())
