import type { ImageFit, ImageKind } from "@/lib/bindings";
import { imageAt, useCoverTransparent, useSoftCover } from "@/lib/cover";

/**
 * 世界背景图的那一层（首页铺满整窗、首页流畅模式、实例卡片、实例页横幅、世界卡片共用，D17d）。
 * 放在一个 `relative` / `absolute` 的容器里，图都是绝对定位铺满容器：
 * - 截图、自动封面、横的自选图：铺满，多出来的裁掉
 * - 「完整显示」的自选图（立绘、竖图）：整张靠右。自带底色的画，后面垫着它自己缩成 8×8 再放大的一片颜色；
 *   透明底的立绘不垫（会多出一个糊掉的影子），从它那边散开一层主色的光（主色就是从这张图里取的）
 * - 只有 64×64 小图标：只铺它的颜色（图标本身由用的地方按像素清楚地画出来）
 * 那片颜色只是图片缩放，不是模糊滤镜，老电脑也不费劲。
 */
export function WorldPicture({
  url,
  kind,
  fit,
  width,
  className = "",
  figure = true,
  idleSampling = false,
  onLoad,
}: {
  url: string;
  kind: ImageKind | null;
  fit: ImageFit;
  /** 要多宽的图（CSS 像素），后端按这个缩小 */
  width: number;
  /** 加在主图上（悬停放大、淡入这类） */
  className?: string;
  /**
   * 完整显示时画不画图本身：实例页横幅又扁又被按钮占着右边，立绘只会缩成一个小影子，
   * 那里只留它的颜色（和小图标在横幅里一样，只当背景）
   */
  figure?: boolean;
  idleSampling?: boolean;
  onLoad?: () => void;
}) {
  const icon = kind === "icon";
  const contain = kind === "custom" && fit === "contain";
  const transparent = useCoverTransparent(url, contain, idleSampling);
  const soft = useSoftCover(url, icon || contain, idleSampling);
  return (
    <>
      {contain && transparent ? (
        <div className="absolute inset-0 bg-radial-[at_78%_45%] from-accent/30 via-accent/10 via-45% to-transparent to-80%" />
      ) : (
        soft && (
          <img src={soft} alt="" aria-hidden className="absolute inset-0 size-full object-cover" />
        )
      )}
      {!icon && (figure || !contain) && (
        <img
          src={imageAt(url, width)}
          alt=""
          aria-hidden
          loading="lazy"
          decoding="async"
          onLoad={onLoad}
          className={`absolute inset-0 size-full ${contain ? "object-contain object-right" : "object-cover"} ${className}`}
        />
      )}
    </>
  );
}
