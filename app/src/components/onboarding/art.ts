import type { OnboardingStep } from "@/stores/accounts";
import type { SceneKind } from "../art/SceneBackground";

/** O1–O4 纯 2D 柔光矢量背景，保留步骤预览在前景。O3 的画面同时呈现白天与夜晚。 */
export const SCENE_ART: Partial<Record<OnboardingStep, SceneKind>> = {
  find: "find",
  account: "account",
  look: "look",
  ready: "ready",
};
