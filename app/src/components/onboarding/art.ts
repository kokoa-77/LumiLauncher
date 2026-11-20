import accountDay from "@/assets/art/account-day.webp";
import accountNight from "@/assets/art/account-night.webp";
import appearance from "@/assets/art/appearance.webp";
import findDay from "@/assets/art/find-day.webp";
import findNight from "@/assets/art/find-night.webp";
import readyDay from "@/assets/art/ready-day.webp";
import readyNight from "@/assets/art/ready-night.webp";
import type { OnboardingStep } from "@/stores/accounts";
import type { SceneArt } from "../art/SceneBackground";

/** O1–O4，保留步骤预览在前景。O3 的画面本身同时有白天和夜晚。 */
export const SCENE_ART: Partial<Record<OnboardingStep, SceneArt>> = {
  find: { night: findNight, day: findDay },
  account: { night: accountNight, day: accountDay },
  look: { night: appearance },
  ready: { night: readyNight, day: readyDay },
};
