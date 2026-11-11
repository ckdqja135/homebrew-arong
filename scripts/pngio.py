import zlib, struct
def read_png(path):
    b = open(path, 'rb').read()
    pos = 8; idat = b''
    while pos < len(b):
        n, t = struct.unpack('>I4s', b[pos:pos+8]); d = b[pos+8:pos+8+n]; pos += 12 + n
        if t == b'IHDR': w, h, bd, ct = struct.unpack('>IIBB', d[:10])
        if t == b'IDAT': idat += d
    assert bd == 8 and ct == 6, (path, bd, ct)
    raw = zlib.decompress(idat); bpp = 4; stride = w * bpp
    prev = bytearray(stride); rows = []; p = 0
    for y in range(h):
        f = raw[p]; line = bytearray(raw[p+1:p+1+stride]); p += 1 + stride
        if f:
            for i in range(stride):
                a = line[i-bpp] if i >= bpp else 0; up = prev[i]; c = prev[i-bpp] if i >= bpp else 0
                if f == 1: v = a
                elif f == 2: v = up
                elif f == 3: v = (a + up) >> 1
                else:
                    pa = abs(up - c); pb = abs(a - c); pc = abs(a + up - 2*c)
                    v = a if pa <= pb and pa <= pc else (up if pb <= pc else c)
                line[i] = (line[i] + v) & 255
        rows.append(line); prev = line
    return w, h, [[tuple(r[x*4:x*4+4]) for x in range(w)] for r in rows]
def write_png(path, w, h, px):
    raw = b''.join(b'\x00' + bytes(v for p in row for v in p) for row in px)
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    open(path, 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))
