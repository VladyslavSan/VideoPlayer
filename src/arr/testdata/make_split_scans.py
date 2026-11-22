"""Writes a tiny baseline JPEG, 4:2:0, with each colour in its own scan."""
import struct, sys
W, H = 64, 32
def seg(m, body): return bytes([0xFF, m]) + struct.pack('>H', len(body) + 2) + body
out = b'\xff\xd8'
out += seg(0xDB, bytes([0]) + bytes([1] * 64))
out += seg(0xC0, bytes([8]) + struct.pack('>HH', H, W) + bytes([3, 1, 0x22, 0, 2, 0x11, 0, 3, 0x11, 0]))
# DC table 0: categories 0..11, all 4 bits long. AC table 0: EOB "00", 0x01 "01".
dc = [0, 0, 0, 12] + [0] * 12
out += seg(0xC4, bytes([0x00]) + bytes(dc) + bytes(range(12)))
ac = [0, 2] + [0] * 14
out += seg(0xC4, bytes([0x10]) + bytes(ac) + bytes([0x00, 0x01]))
class Bits:
    def __init__(s): s.v = 0; s.n = 0; s.out = bytearray()
    def put(s, value, n):
        for i in range(n - 1, -1, -1):
            s.v = (s.v << 1) | ((value >> i) & 1); s.n += 1
            if s.n == 8:
                s.out.append(s.v)
                if s.v == 0xFF: s.out.append(0)
                s.v = 0; s.n = 0
    def done(s):
        while s.n: s.put(1, 1)
        return bytes(s.out)
def scan(comp, blocks):
    b = Bits(); prev = 0
    for level in blocks:
        dcv = 8 * (level - 128); d = dcv - prev; prev = dcv
        cat = abs(d).bit_length()
        b.put(cat, 4)
        if cat: b.put(d if d > 0 else d + (1 << cat) - 1, cat)
        b.put(0, 2)  # EOB
    return seg(0xDA, bytes([1, comp, 0x00, 0, 63, 0])) + b.done()
# Y: 8x4 blocks; brighter on the left. Cb/Cr: 4x2 blocks, one colour: red-ish.
ys = [150 if x < 4 else 110 for y in range(4) for x in range(8)]
out += scan(1, ys) + scan(2, [100] * 8) + scan(3, [200] * 8) + b'\xff\xd9'
open(sys.argv[1], 'wb').write(out)
