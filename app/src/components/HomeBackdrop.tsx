import { motion } from "motion/react";
import { useEffect, useState } from "react";
import { LightField } from "@/components/art/LightField";
import { SceneBackground } from "@/components/art/SceneBackground";
import { duration, ease } from "@/lib/motion";
import { type Backdrop, useEffectsLevel } from "@/stores/app";
import { WorldPicture } from "./WorldPicture";

/** 窗口变宽时才换一张更大的图（缩小不换，免得拖动窗口时一直重新取图） */
function useWindowWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    let timer = 0;
    const onResize = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setWidth((w) => Math.max(w, window.innerWidth)), 300);
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      window.clearTimeout(timer);
    };
  }, []);
  return width;
}

/**
 * 首页铺满整个窗口的背景，在标题栏和侧栏后面。没有世界封面时使用 Lumi 插画。
 * 图片按窗口宽度缩小后再取（省内存）。三层渐变（上：标题栏，左：侧栏，下：世界名和按钮）用的是窗口底色：
 * 浅色是白天，截图融进浅色底；深色是夜晚，整张图再压一层夜空蓝。不用实时模糊。
 * 截图、自动封面、自己选的立绘、只有小图标的世界各怎么画，见 `WorldPicture`。
 */
export function HomeBackdrop({ backdrop, ready }: { backdrop: Backdrop | null; ready: boolean }) {
  const animated = useEffectsLevel() === "rich";
  const width = useWindowWidth();
  const [loaded, setLoaded] = useState(false);
  return (
    <motion.div
      aria-hidden
      className="pointer-events-none absolute inset-0 overflow-hidden"
      data-home-backdrop={backdrop?.kind ?? "default"}
      initial={animated ? { opacity: 0 } : false}
      animate={{
        opacity: 1,
        transition: { duration: animated ? duration.page : 0, ease: ease.out },
      }}
      exit={{
        opacity: 0,
        transition: { duration: animated ? duration.page : 0, ease: ease.inOut },
      }}
    >
      {backdrop ? (
        <>
          <WorldPicture
            url={backdrop.url}
            kind={backdrop.kind}
            fit={backdrop.fit}
            width={width}
            onLoad={() => setLoaded(true)}
            className={`transition-opacity duration-(--motion-page) ease-out ${
              backdrop.kind !== "icon" && backdrop.fit !== "contain" ? "lumi-hero-breathe" : ""
            } ${loaded ? "opacity-100" : "opacity-0"}`}
          />
          <div className="absolute inset-0 bg-hero-tint" />
          {/* 有世界截图的首页在精美模式下仍保留少量淡微光尘埃，颜色随世界主色变化 */}
          {ready && animated && (
            <LightField count={7} seed={47} area="low" className="absolute inset-0 opacity-65" />
          )}
        </>
      ) : (
        <div className="absolute inset-0 bg-linear-to-b from-sky-top to-sky-bottom">
          {ready && <SceneBackground kind="home" showInLite />}
          {ready && animated && (
            <LightField count={18} seed={41} area="low" className="absolute inset-0" />
          )}
        </div>
      )}
      <div className="absolute inset-x-0 top-0 h-32 bg-linear-to-b from-hero-scrim/80 via-hero-scrim/35 via-55% to-transparent" />
      <div className="absolute inset-y-0 left-0 w-64 bg-linear-to-r from-hero-scrim/75 via-hero-scrim/30 via-55% to-transparent" />
      {/* 非线性多段缓动遮罩：消除高对比截图上的渐变断层，同时保持画面通透 */}
      <div className="absolute inset-x-0 bottom-0 h-[56%] bg-linear-to-t from-hero-scrim from-0% via-hero-scrim/72 via-28% via-hero-scrim/28 via-62% to-transparent" />
    </motion.div>
  );
}
