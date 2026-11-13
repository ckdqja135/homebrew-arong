#!/usr/bin/env python3
"""에셋 갱신용(일회성 개발 도구): 이미 있는 옷 시트의 옷 색만 다른 옷의 색으로 바꿔 새 시트를 만든다.

사용: python3 -I scripts/recolor_outfit.py <원본 옷 id> <새 옷 id> <색 견본 시트> <단계>_<동작> [...]
(색 견본 시트는 새 옷 폴더에 이미 있는 시트 이름이다. 예: adult_paw_left)

옷 픽셀은 "기본 그림과 달라진 픽셀"로 찾는다. 그중 색이 있는 픽셀만 밝기 순위를 맞춰 견본 옷의 색으로 바꾸고,
흰 줄무늬 같은 무채색 픽셀과 모양·위치는 그대로 둔다.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngio import read_png, write_png

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")


def luma(p):
    return 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]


def colored(p):
    return max(p[:3]) - min(p[:3]) >= 14


def garment(outfit, name):
    """(시트, 옷 픽셀 좌표 목록): 기본 그림과 다르고 색이 있는 불투명 픽셀."""
    w, h, px = read_png(os.path.join(ROOT, "outfits", outfit, f"{name}.png"))
    _, _, base = read_png(os.path.join(ROOT, f"{name}.png"))
    spots = [(x, y) for y in range(h) for x in range(w) if px[y][x][3] > 127 and px[y][x] != base[y][x] and colored(px[y][x])]
    return w, h, px, spots


def main():
    source, target, sample, names = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
    _, _, spx, sspots = garment(target, sample)
    ramp = sorted((spx[y][x] for x, y in sspots), key=luma)
    for name in names:
        w, h, px, spots = garment(source, name)
        order = sorted(spots, key=lambda s: luma(px[s[1]][s[0]]))
        for rank, (x, y) in enumerate(order):
            c = ramp[rank * (len(ramp) - 1) // max(len(order) - 1, 1)]
            px[y][x] = (c[0], c[1], c[2], 255)
        os.makedirs(os.path.join(ROOT, "outfits", target), exist_ok=True)
        write_png(os.path.join(ROOT, "outfits", target, f"{name}.png"), w, h, px)
        print(f"{name}: recolored {len(order)} px from {len(ramp)} sample px")


if __name__ == "__main__":
    main()
