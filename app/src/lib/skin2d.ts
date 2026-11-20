// 按皮肤文件的像素画 2D 人物（D19b）：账号弹层的人物、头像、流畅模式下的换皮肤预览都用它。
// 画在很小的画布上（头 8×8、全身 16×32），再用 CSS 放大、像素不糊（image-rendering: pixelated）。
// 皮肤贴图的格子位置是 Minecraft 公开的格式（64×64 新格式、64×32 老格式）。

import type { SkinModel } from "./bindings";

export type Side = "front" | "back";

/** 一块贴图：从皮肤的 (sx, sy) 取 w×h，画到人物的 (dx, dy)；`mirror` 左右翻（老格式的左手左脚用右边的翻过来） */
interface Piece {
  sx: number;
  sy: number;
  w: number;
  h: number;
  dx: number;
  dy: number;
  mirror?: boolean;
  /** 外层（帽子、外套、袖子、裤子），只有 64×64 的新格式有（帽子两种都有） */
  overlay?: boolean;
}

const cache = new Map<string, Promise<HTMLImageElement>>();

/** 读皮肤图片（同一张只读一次） */
export function loadSkinImage(url: string): Promise<HTMLImageElement> {
  let found = cache.get(url);
  if (!found) {
    found = new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = () => reject(new Error("皮肤图片读不出来"));
      image.src = url;
    });
    cache.set(url, found);
    // 只留最近的几张（切账号来回看几张就够）
    if (cache.size > 16) {
      const oldest = cache.keys().next().value;
      if (oldest !== undefined) cache.delete(oldest);
    }
  }
  return found;
}

/** 全身的贴图块（人物画布 16×32：头 8、身体 12、腿 12） */
function bodyPieces(model: SkinModel, side: Side, legacy: boolean): Piece[] {
  const arm = model === "slim" ? 3 : 4;
  if (side === "front") {
    // 正面：玩家的右手在画面左边
    return [
      { sx: 8, sy: 8, w: 8, h: 8, dx: 4, dy: 0 },
      { sx: 40, sy: 8, w: 8, h: 8, dx: 4, dy: 0, overlay: true },
      { sx: 20, sy: 20, w: 8, h: 12, dx: 4, dy: 8 },
      { sx: 20, sy: 36, w: 8, h: 12, dx: 4, dy: 8, overlay: true },
      { sx: 44, sy: 20, w: arm, h: 12, dx: 4 - arm, dy: 8 },
      { sx: 44, sy: 36, w: arm, h: 12, dx: 4 - arm, dy: 8, overlay: true },
      legacy
        ? { sx: 44, sy: 20, w: arm, h: 12, dx: 12, dy: 8, mirror: true }
        : { sx: 36, sy: 52, w: arm, h: 12, dx: 12, dy: 8 },
      { sx: 52, sy: 52, w: arm, h: 12, dx: 12, dy: 8, overlay: true },
      { sx: 4, sy: 20, w: 4, h: 12, dx: 4, dy: 20 },
      { sx: 4, sy: 36, w: 4, h: 12, dx: 4, dy: 20, overlay: true },
      legacy
        ? { sx: 4, sy: 20, w: 4, h: 12, dx: 8, dy: 20, mirror: true }
        : { sx: 20, sy: 52, w: 4, h: 12, dx: 8, dy: 20 },
      { sx: 4, sy: 52, w: 4, h: 12, dx: 8, dy: 20, overlay: true },
    ];
  }
  // 背面：玩家的右手在画面右边
  const armBack = 44 + arm + 4;
  return [
    { sx: 24, sy: 8, w: 8, h: 8, dx: 4, dy: 0 },
    { sx: 56, sy: 8, w: 8, h: 8, dx: 4, dy: 0, overlay: true },
    { sx: 32, sy: 20, w: 8, h: 12, dx: 4, dy: 8 },
    { sx: 32, sy: 36, w: 8, h: 12, dx: 4, dy: 8, overlay: true },
    { sx: armBack, sy: 20, w: arm, h: 12, dx: 12, dy: 8 },
    { sx: armBack, sy: 36, w: arm, h: 12, dx: 12, dy: 8, overlay: true },
    legacy
      ? { sx: armBack, sy: 20, w: arm, h: 12, dx: 4 - arm, dy: 8, mirror: true }
      : { sx: 36 + arm + 4, sy: 52, w: arm, h: 12, dx: 4 - arm, dy: 8 },
    { sx: 52 + arm + 4, sy: 52, w: arm, h: 12, dx: 4 - arm, dy: 8, overlay: true },
    { sx: 12, sy: 20, w: 4, h: 12, dx: 8, dy: 20 },
    { sx: 12, sy: 36, w: 4, h: 12, dx: 8, dy: 20, overlay: true },
    legacy
      ? { sx: 12, sy: 20, w: 4, h: 12, dx: 4, dy: 20, mirror: true }
      : { sx: 28, sy: 52, w: 4, h: 12, dx: 4, dy: 20 },
    { sx: 12, sy: 52, w: 4, h: 12, dx: 4, dy: 20, overlay: true },
  ];
}

function drawPieces(ctx: CanvasRenderingContext2D, image: HTMLImageElement, pieces: Piece[]) {
  const legacy = image.naturalHeight < 64;
  ctx.imageSmoothingEnabled = false;
  for (const piece of pieces) {
    // 老格式只有帽子这一层外层（y < 32）
    if (piece.overlay && legacy && piece.sy >= 32) continue;
    if (piece.mirror) {
      ctx.save();
      ctx.translate(piece.dx + piece.w, piece.dy);
      ctx.scale(-1, 1);
      ctx.drawImage(image, piece.sx, piece.sy, piece.w, piece.h, 0, 0, piece.w, piece.h);
      ctx.restore();
    } else {
      ctx.drawImage(
        image,
        piece.sx,
        piece.sy,
        piece.w,
        piece.h,
        piece.dx,
        piece.dy,
        piece.w,
        piece.h,
      );
    }
  }
}

/** 全身画到 16×32 的画布上 */
export function drawBody(
  canvas: HTMLCanvasElement,
  image: HTMLImageElement,
  model: SkinModel,
  side: Side,
) {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  canvas.width = 16;
  canvas.height = 32;
  ctx.clearRect(0, 0, 16, 32);
  drawPieces(ctx, image, bodyPieces(model, side, image.naturalHeight < 64));
}

/** 头（脸 + 帽子那层）画到 8×8 的画布上 */
export function drawHead(canvas: HTMLCanvasElement, image: HTMLImageElement) {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  canvas.width = 8;
  canvas.height = 8;
  ctx.clearRect(0, 0, 8, 8);
  drawPieces(ctx, image, [
    { sx: 8, sy: 8, w: 8, h: 8, dx: 0, dy: 0 },
    { sx: 40, sy: 8, w: 8, h: 8, dx: 0, dy: 0, overlay: true },
  ]);
}

/** 测试用：某一面要从皮肤的哪些位置取（只看位置，不画） */
export function piecesFor(model: SkinModel, side: Side, legacy: boolean) {
  return bodyPieces(model, side, legacy);
}
