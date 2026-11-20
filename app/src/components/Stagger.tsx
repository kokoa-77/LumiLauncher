import { motion, type Variants } from "motion/react";
import type { ReactNode } from "react";
import { duration, ease } from "@/lib/motion";

const container: Variants = {
  hidden: {},
  show: { transition: { staggerChildren: 0.03 } },
};

const item: Variants = {
  hidden: { opacity: 0, y: 8 },
  show: { opacity: 1, y: 0, transition: { duration: duration.enter, ease: ease.out } },
};

/** 子元素依次错开入场。 */
export function Stagger({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <motion.div className={className} variants={container} initial="hidden" animate="show">
      {children}
    </motion.div>
  );
}

export function StaggerItem({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <motion.div className={className} variants={item}>
      {children}
    </motion.div>
  );
}
