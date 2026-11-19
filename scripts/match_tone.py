#!/usr/bin/env python3
"""새로 받은 시트의 밝기를 기준 시트에 맞춘다(색조는 그대로 두고 밝기 분포만 옮긴다).

    python3 scripts/match_tone.py <받은 시트> <기준 시트> <출력>

AI로 다시 그린 시트는 기존 그림보다 전체적으로 밝거나 어둡게 나오는 일이 있다.
불투명 픽셀의 밝기 분포를 기준 시트의 분포에 맞춰(히스토그램 매칭) 같은 강아지로 보이게 한다.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngio import read_png, write_png


def lum(q):
    return 0.299 * q[0] + 0.587 * q[1] + 0.114 * q[2]


def cdf(pixels):
    hist = [0] * 256
    for q in pixels:
        hist[int(lum(q) + 0.5)] += 1
    total, run, out = sum(hist), 0, []
    for n in hist:
        run += n
        out.append(run / total)
    return out


def main(src, ref, dst):
    w, h, px = read_png(src)
    _, _, ref_px = read_png(ref)
    solid = lambda rows: [q for row in rows for q in row if q[3] > 127]
    a, b = cdf(solid(px)), cdf(solid(ref_px))
    # 받은 시트의 밝기 v → 기준 시트에서 같은 순위에 있는 밝기.
    table, j = [], 0
    for v in range(256):
        while j < 255 and b[j] < a[v]:
            j += 1
        table.append(j)
    out = []
    for row in px:
        new = []
        for q in row:
            if q[3] == 0:
                new.append(q)
                continue
            v = lum(q)
            k = table[int(v + 0.5)] / v if v > 0.5 else 1.0
            new.append((min(255, int(q[0] * k + 0.5)), min(255, int(q[1] * k + 0.5)), min(255, int(q[2] * k + 0.5)), q[3]))
        out.append(new)
    write_png(dst, w, h, out)


if __name__ == "__main__":
    main(*sys.argv[1:4])
