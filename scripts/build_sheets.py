#!/usr/bin/env python3
"""에셋 갱신용(일회성 개발 도구): 단계별 8모션 묶음을 앱이 쓰는 시트로 정리해 assets/에 둔다.

사용: python3 -I scripts/build_sheets.py <baby 폴더> <growing 폴더> <adult 폴더>
(각 폴더는 arong_<stage>_8motions.zip을 푼 곳)

하는 일은 방향 맞추기뿐이다. 앱은 "왼쪽을 보는 그림"을 기준으로 삼고 오른쪽은 뒤집어 그린다.
- 걷기: 좌우 시트가 서로 거울상인데 파일 이름과 실제 방향이 단계마다 다르다. 실제로 왼쪽을 보는 쪽을 고른다.
- 오른발 주기: 오른쪽을 보고 그려져 있어 좌우를 뒤집어 왼쪽을 보게 한다.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngio import read_png, write_png

CELL = 128
# (앱에서 쓰는 이름, 원본 파일 이름, 좌우 뒤집기)
PLAN = {
    "baby": [("walk", "walk_left", False)],
    "growing": [("walk", "walk_right", False)],
    "adult": [("walk", "walk_right", False)],
}
COMMON = [
    ("paw_left", "paw_left", False),
    ("paw_right", "paw_right", True),
    ("lie_down", "lie_down", False),
    ("sit", "sit", False),
    ("sit_up", "sit_to_stand", False),
    ("roll", "roll", False),
]


def mirror_frames(w, px):
    out = []
    for row in px:
        line = []
        for f in range(w // CELL):
            line += list(reversed(row[f * CELL:(f + 1) * CELL]))
        out.append(line)
    return out


def main():
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
    for stage, src in zip(("baby", "growing", "adult"), sys.argv[1:4]):
        for name, original, flip in PLAN[stage] + COMMON:
            w, h, px = read_png(os.path.join(src, f"{stage}_{original}_sheet.png"))
            assert h == CELL and w % CELL == 0, (stage, original, w, h)
            if flip:
                px = mirror_frames(w, px)
            write_png(os.path.join(out, f"{stage}_{name}.png"), w, h, px)
            print(stage, name, w // CELL, "frames")


if __name__ == "__main__":
    main()
