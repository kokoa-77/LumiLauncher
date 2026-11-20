import { describe, expect, it } from "vitest";
import { piecesFor } from "./skin2d";

describe("2D 人物的贴图位置", () => {
  it("正面：玩家的右手在画面左边，经典 4 像素宽、纤细 3 像素宽", () => {
    const classic = piecesFor("classic", "front", false);
    const slim = piecesFor("slim", "front", false);
    const rightArm = (pieces: typeof classic) => pieces.find((p) => p.sx === 44 && p.sy === 20);
    expect(rightArm(classic)).toMatchObject({ w: 4, dx: 0, dy: 8 });
    expect(rightArm(slim)).toMatchObject({ w: 3, dx: 1, dy: 8 });
  });

  it("老格式（64×32）没有左手左脚，用右边的翻过来", () => {
    const legacy = piecesFor("classic", "front", true);
    const mirrored = legacy.filter((p) => p.mirror);
    expect(mirrored).toHaveLength(2);
    expect(mirrored.map((p) => [p.sx, p.dx])).toEqual([
      [44, 12],
      [4, 8],
    ]);
  });

  it("背面取的是背面那一格：纤细手臂的背面在 x = 51", () => {
    const back = piecesFor("slim", "back", false);
    expect(back.find((p) => p.sy === 20 && p.dx === 12)).toMatchObject({ sx: 51, w: 3 });
    expect(back.find((p) => p.sy === 8 && !p.overlay)).toMatchObject({ sx: 24, dx: 4 });
  });

  it("每一块都在 16×32 的画布里", () => {
    for (const model of ["classic", "slim"] as const) {
      for (const side of ["front", "back"] as const) {
        for (const piece of piecesFor(model, side, false)) {
          expect(piece.dx).toBeGreaterThanOrEqual(0);
          expect(piece.dx + piece.w).toBeLessThanOrEqual(16);
          expect(piece.dy + piece.h).toBeLessThanOrEqual(32);
        }
      }
    }
  });
});
