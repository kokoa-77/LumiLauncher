// 占位头像的像素。这里的颜色是「头像图片本身」的颜色（相当于一张皮肤图），不是界面颜色，
// 所以不走设计令牌。做了导入皮肤之后，头像直接从皮肤文件里取那 8×8 的脸。

const HAIR_HUES = [25, 40, 60, 210, 280, 330, 0, 140];

function hash(text: string): number {
  let h = 0;
  for (const ch of text) h = (h * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  return h;
}

/** 8×8 像素脸：头发、皮肤、眼睛。颜色由名字决定，同一个名字每次都一样 */
export function headPixels(name: string): [number, number, number, number, string][] {
  const h = hash(name || "Player");
  const hair = `oklch(${42 + (h % 3) * 12}% 0.08 ${HAIR_HUES[h % HAIR_HUES.length]})`;
  const skin = `oklch(${78 + ((h >> 3) % 3) * 5}% 0.06 55)`;
  const white = "oklch(98% 0 0)";
  const eye = "oklch(35% 0.07 260)";
  return [
    [0, 0, 8, 8, skin],
    [0, 0, 8, 2, hair],
    [0, 2, 1, 2, hair],
    [7, 2, 1, 2, hair],
    [1, 4, 2, 1, white],
    [5, 4, 2, 1, white],
    [2, 4, 1, 1, eye],
    [5, 4, 1, 1, eye],
    [3, 6, 2, 1, "oklch(55% 0.08 30)"],
  ];
}
