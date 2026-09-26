#!/usr/bin/env python3
"""生成应用图标 (纯标准库, 无需 PIL)。渲染 2x 超采样后盒式降采样抗锯齿。"""
import struct, zlib, os, sys

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")

def png_chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

def write_png(path, w, h, rgba):
    rows = b"".join(b"\x00" + bytes(rgba[y*w*4:(y+1)*w*4]) for y in range(h))
    png = (b"\x89PNG\r\n\x1a\n"
           + png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
           + png_chunk(b"IDAT", zlib.compress(rows, 9))
           + png_chunk(b"IEND", b""))
    with open(path, "wb") as f:
        f.write(png)

def inside_rounded(x, y, x0, y0, x1, y1, r):
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

def mix(c1, c2, t):
    return tuple(round(a + (b - a) * t) for a, b in zip(c1, c2))

def render(size):
    """返回 RGBA bytes, 尺寸 size x size。内部以 2x 超采样绘制。"""
    S = size * 2
    buf = bytearray(S * S * 4)
    m = S * 0.04            # 画布边距
    rr = S * 0.22           # 圆角半径
    bg1, bg2 = (36, 48, 63), (16, 22, 29)
    keyc, accent = (232, 236, 241), (34, 211, 238)
    # 键盘网格: 4 行 12 列
    gx0, gx1 = S * 0.14, S * 0.86
    gy0, gy1 = S * 0.24, S * 0.80
    cols, rows = 12, 4
    gap = (gx1 - gx0) * 0.025
    kw = ((gx1 - gx0) - gap * (cols - 1)) / cols
    kh = ((gy1 - gy0) - gap * (rows - 1)) / rows
    kr = min(kw, kh) * 0.20

    def key_rect(row, col):
        x0 = gx0 + col * (kw + gap)
        y0 = gy0 + row * (kh + gap)
        return x0, y0, x0 + kw, y0 + kh

    # 底行特殊: space 跨列 2..10, 高亮
    for py in range(S):
        t = py / S
        base = mix(bg1, bg2, t)
        for px in range(S):
            i = (py * S + px) * 4
            if inside_rounded(px, py, m, m, S - m, S - m, rr):
                c = base
                # 普通键 (行 0-2 + 底行两端)
                for row in range(rows):
                    if row == 3:
                        spans = [(0, 1, False), (10, 11, False), (2, 9, True)]
                    else:
                        spans = [(0, cols - 1, False)]
                    for c0, c1, is_accent in spans:
                        for col in range(c0, c1 + 1):
                            x0, y0, x1, y1 = key_rect(row, col)
                            if inside_rounded(px, py, x0, y0, x1, y1, kr):
                                c = accent if is_accent else keyc
                buf[i:i+3] = bytes(c)
                buf[i+3] = 255
    # 盒式降采样 2x
    out = bytearray(size * size * 4)
    for y in range(size):
        for x in range(size):
            for ch in range(4):
                s = 0
                for dy in range(2):
                    for dx in range(2):
                        s += buf[((y*2+dy)*S + (x*2+dx))*4 + ch]
                out[(y*size + x)*4 + ch] = s // 4
    return out

def downscale(rgba, w, h, tw, th):
    out = bytearray(tw * th * 4)
    fx, fy = w / tw, h / th
    for y in range(th):
        for x in range(tw):
            for ch in range(4):
                s = 0
                cnt = 0
                for yy in range(int(y*fy), max(int(y*fy)+1, int((y+1)*fy))):
                    for xx in range(int(x*fx), max(int(x*fx)+1, int((x+1)*fx))):
                        s += rgba[(yy*w + xx)*4 + ch]; cnt += 1
                out[(y*tw + x)*4 + ch] = s // cnt
    return out

def main():
    os.makedirs(OUT, exist_ok=True)
    img = render(512)
    write_png(os.path.join(OUT, "icon.png"), 512, 512, img)
    for name, sz in [("128x128.png", 128), ("128x128@2x.png", 256), ("32x32.png", 32)]:
        write_png(os.path.join(OUT, name), sz, sz, downscale(img, 512, 512, sz, sz))
    print("icons written to", os.path.abspath(OUT))

if __name__ == "__main__":
    sys.exit(main())
