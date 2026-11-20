import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { duration, ease, spring } from "@/lib/motion";
import { IconButton } from "./ui/IconButton";

/** 大图：地址和图下面的一行字（可以没有） */
export interface LightboxImage {
  url: string;
  title?: string | null;
}

/** 点开一张图：铺满窗口看大图，左右切换，Esc 或点空白处关掉 */
export function Lightbox({
  images,
  index,
  onIndex,
  onClose,
}: {
  images: LightboxImage[];
  index: number | null;
  onIndex: (i: number) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  useEffect(() => {
    if (index === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key === "ArrowRight") onIndex((index + 1) % images.length);
      if (event.key === "ArrowLeft") onIndex((index - 1 + images.length) % images.length);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, images.length, onIndex, onClose]);
  const image = index === null ? undefined : images[index];
  return (
    <AnimatePresence>
      {image && index !== null && (
        <motion.div
          className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-scrim p-10"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1, transition: { duration: duration.overlay, ease: ease.out } }}
          exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          onClick={onClose}
        >
          <motion.img
            key={image.url}
            src={image.url}
            alt={image.title ?? ""}
            className="max-h-[80vh] max-w-full rounded-panel object-contain shadow-dialog"
            initial={{ opacity: 0, scale: 0.98 }}
            animate={{ opacity: 1, scale: 1, transition: spring.gentle }}
            onClick={(event) => event.stopPropagation()}
          />
          {image.title && <p className="text-on-hero text-sm">{image.title}</p>}
          <div className="absolute top-4 right-4">
            <IconButton label={t("discover.project.close")} variant="hero" onClick={onClose}>
              <X className="size-4" aria-hidden />
            </IconButton>
          </div>
          {images.length > 1 && (
            <>
              <div className="-translate-y-1/2 absolute top-1/2 left-4">
                <IconButton
                  label={t("discover.project.prev")}
                  variant="hero"
                  onClick={(event) => {
                    event.stopPropagation();
                    onIndex((index - 1 + images.length) % images.length);
                  }}
                >
                  <ChevronLeft className="size-5" aria-hidden />
                </IconButton>
              </div>
              <div className="-translate-y-1/2 absolute top-1/2 right-4">
                <IconButton
                  label={t("discover.project.next")}
                  variant="hero"
                  onClick={(event) => {
                    event.stopPropagation();
                    onIndex((index + 1) % images.length);
                  }}
                >
                  <ChevronRight className="size-5" aria-hidden />
                </IconButton>
              </div>
            </>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
