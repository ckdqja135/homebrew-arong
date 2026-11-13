#!/usr/bin/env python3
"""에셋 갱신용(일회성 개발 도구): 옷 입은 그림 묶음을 앱이 쓰는 시트로 정리해 assets/outfits/<옷 id>/에 둔다.

사용: python3 -I scripts/build_outfit.py <옷 id> <단계(baby|growing|adult)> <시트 폴더>
(폴더에는 <단계>_<동작>_sheet.png 가 들어 있다. 있는 동작만 처리하고, 없는 동작은 앱이 기본 그림을 쓴다.)

옷 입은 그림은 기본 그림(assets/<단계>_<동작>.png)과 프레임 수·위치·윤곽이 같아야 한다.
방향은 파일 이름을 믿지 않고 기본 그림과 윤곽을 비교해서 정한다(거울상 쪽이 맞으면 좌우를 뒤집는다).
크기 조정·여백 제거·재정렬은 하지 않는다. 원본 파일은 건드리지 않는다.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngio import read_png, write_png

CELL = 128
# 원본 파일 이름 → 앱에서 쓰는 이름
NAMES = {
    "walk": "walk", "sit_up": "sit_up", "walk_left": "walk", "walk_right": "walk", "paw_left": "paw_left", "paw_right": "paw_right",
    "lie_down": "lie_down", "sit": "sit", "sit_to_stand": "sit_up", "roll": "roll", "pant": "pant", "look": "look",
}


def mirror_frames(w, px):
    return [[p for f in range(w // CELL) for p in reversed(row[f * CELL:(f + 1) * CELL])] for row in px]


def overlap(a, b):
    """프레임별 윤곽 일치도(IoU)의 최솟값."""
    worst = 1.0
    for f in range(len(a[0]) // CELL):
        inter = union = 0
        for ra, rb in zip(a, b):
            for x in range(f * CELL, (f + 1) * CELL):
                p, q = ra[x][3] > 127, rb[x][3] > 127
                inter += p and q
                union += p or q
        worst = min(worst, inter / max(union, 1))
    return worst


def main():
    outfit, stage, src = sys.argv[1:4]
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
    out = os.path.join(root, "outfits", outfit)
    os.makedirs(out, exist_ok=True)
    done = set()
    for original, name in NAMES.items():
        path = os.path.join(src, f"{stage}_{original}_sheet.png")
        if name in done or not os.path.exists(path):
            continue
        w, h, px = read_png(path)
        bw, bh, base = read_png(os.path.join(root, f"{stage}_{name}.png"))
        assert (w, h) == (bw, bh) and h == CELL, (original, w, h, bw, bh)
        same, flipped = overlap(px, base), overlap(mirror_frames(w, px), base)
        flip = flipped > same
        assert max(same, flipped) >= 0.97, (original, same, flipped)
        if flip:
            px = mirror_frames(w, px)
        soft = sum(1 for row in px for p in row if 0 < p[3] < 255)
        write_png(os.path.join(out, f"{stage}_{name}.png"), w, h, px)
        done.add(name)
        print(f"{stage} {name}: {w // CELL} frames, flip={flip}, outline match {max(same, flipped):.3f}, semi-transparent px {soft}")


if __name__ == "__main__":
    main()
