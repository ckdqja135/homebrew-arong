#!/usr/bin/env python3
"""에셋 갱신용(일회성 개발 도구): 동작별 가로 시트 9장을 단계별 아틀라스 한 장으로 합친다.

사용: python3 -I scripts/build_atlases.py <압축 푼 에셋 폴더>

v2 에셋(arong_27_gifs_v2.zip)의 시트·GIF는 털의 어두운 색 세 가지가 밝은 색으로
잘못 바뀌어 저장돼 있다(같은 묶음의 *_base.png는 정상). 그래서 합치면서 그 세 색을
원래의 어두운 털 색으로 되돌린다. 몸에서 떨어진 작은 조각(반짝이 등)은 건드리지 않는다.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngio import read_png, write_png

CELL, FRAMES = 112, 8
# 아틀라스의 행 순서(sprites::Motion과 같다).
MOTIONS = ["walk", "affection", "run", "leash", "eat", "pant", "sit", "paw", "look"]
# 잘못 저장된 밝은 색 -> 원래 털 색. 마지막 항목은 원본의 두 색이 하나로 합쳐져 있어 중간값을 쓴다.
FUR_FIX = {
    (245, 203, 198): (15, 12, 13),
    (246, 232, 211): (26, 23, 25),
    (253, 247, 232): (46, 40, 39),
}


def fix_frame(px, x0):
    """프레임 한 칸에서 가장 큰 덩어리(몸)에 속한 픽셀만 털 색을 되돌린다."""
    seen = [[False] * CELL for _ in range(CELL)]
    biggest = []
    for sy in range(CELL):
        for sx in range(CELL):
            if seen[sy][sx] or px[sy][x0 + sx][3] == 0:
                continue
            part, stack = [], [(sx, sy)]
            seen[sy][sx] = True
            while stack:
                x, y = stack.pop()
                part.append((x, y))
                for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                    if 0 <= nx < CELL and 0 <= ny < CELL and not seen[ny][nx] and px[ny][x0 + nx][3] > 0:
                        seen[ny][nx] = True
                        stack.append((nx, ny))
            if len(part) > len(biggest):
                biggest = part
    for x, y in biggest:
        r, g, b, a = px[y][x0 + x]
        if (r, g, b) in FUR_FIX:
            px[y][x0 + x] = FUR_FIX[(r, g, b)] + (a,)


def main():
    src = sys.argv[1]
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
    for stage in ("baby", "growing", "adult"):
        atlas = []
        for motion in MOTIONS:
            w, h, px = read_png(os.path.join(src, stage, f"{stage}_{motion}_spritesheet.png"))
            assert (w, h) == (CELL * FRAMES, CELL), (stage, motion, w, h)
            for frame in range(FRAMES):
                fix_frame(px, frame * CELL)
            atlas.extend(px)
        write_png(os.path.join(out, f"{stage}.png"), CELL * FRAMES, CELL * len(MOTIONS), atlas)
        print(stage, "ok")


if __name__ == "__main__":
    main()
