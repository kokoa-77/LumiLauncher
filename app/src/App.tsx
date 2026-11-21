import { getCurrentWindow } from "@tauri-apps/api/window";
import { MotionConfig } from "motion/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RouterProvider } from "react-router/dom";
import { Toaster, toast } from "sonner";
import { unwrap } from "./lib/api";
import { commands } from "./lib/bindings";
import { applyEffects, isLowEnd, probeRenderer, rememberLowEnd } from "./lib/effects";
import { logError, reportError } from "./lib/report";
import {
  applyBackdrop,
  applyReduceMotion,
  applyTheme,
  type ResolvedTheme,
  resolveTheme,
  systemPrefersDark,
  watchSystemTheme,
} from "./lib/theme";
import { applyAccent } from "./lib/themeEngine";
import { router } from "./router";
import { useAccountStore } from "./stores/accounts";
import { useAppStore, useEffectsLevel } from "./stores/app";
import { initGameEvents } from "./stores/game";
import { useUpdateStore } from "./stores/updates";

export function App() {
  const { t, i18n } = useTranslation();
  const boot = useAppStore((s) => s.boot);
  const loadAppDetails = useAppStore((s) => s.loadAppDetails);
  const effectsMode = useAppStore((s) => s.settings?.appearance.effects ?? "auto");
  const info = useAppStore((s) => s.info);
  const themeMode = useAppStore((s) => s.settings?.appearance.theme ?? "system");
  const reduceMotion = useAppStore((s) => s.settings?.appearance.reduceMotion ?? false);
  const setLowEnd = useAppStore((s) => s.setLowEnd);
  const effects = useEffectsLevel();
  const [booted, setBooted] = useState(false);
  const [theme, setTheme] = useState<ResolvedTheme>(() =>
    resolveTheme("system", systemPrefersDark()),
  );

  // 读取版本信息和设置；失败也要继续显示界面（用默认值），并报告错误
  useEffect(() => {
    boot()
      .catch((error: unknown) => reportError(error, "启动器初始化"))
      .finally(() => setBooted(true));
  }, [boot]);

  // 同步当前界面语言到后端（供「默认游戏语言：跟随启动器」使用）
  useEffect(() => {
    void commands.appSetUiLanguage(i18n.language).catch(() => undefined);
  }, [i18n.language]);

  // 订阅后端事件（任务进度、游戏状态、日志），读取实例列表
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    initGameEvents()
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch((error: unknown) => reportError(error, "订阅后端事件"));
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  // 读取账户列表和 Microsoft 登录可用状态
  useEffect(() => {
    void useAccountStore.getState().refresh();
  }, []);

  // 主题：手动选择或跟随系统，系统切换时实时生效
  useEffect(() => {
    const apply = (prefersDark: boolean) => {
      const resolved = resolveTheme(themeMode, prefersDark);
      applyTheme(resolved);
      setTheme(resolved);
    };
    apply(systemPrefersDark());
    // 窗口自己也换深浅色：Windows 11 的云母背景跟着窗口走，不换的话深色界面底下垫着浅色云母，整片发灰
    getCurrentWindow()
      .setTheme(themeMode === "system" ? null : themeMode)
      .catch((error: unknown) => logError(error, "设置窗口深浅色"));
    return watchSystemTheme(apply);
  }, [themeMode]);

  useEffect(() => applyReduceMotion(reduceMotion), [reduceMotion]);

  // 关了「跟着世界变」：在哪一页都是自己选的主色（开着时由首页、实例页按世界换）
  const followWorld = useAppStore((s) => s.settings?.appearance.accentFollowWorld ?? true);
  const ownHue = useAppStore((s) => s.settings?.appearance.accentHue ?? null);
  const ownChroma = useAppStore((s) => s.settings?.appearance.accentChroma ?? null);
  useEffect(() => {
    if (followWorld || ownHue === null || ownChroma === null) return;
    applyAccent(ownHue, ownChroma);
  }, [followWorld, ownHue, ownChroma]);

  // 画面效果：先用上次的检测结果（避免先按精美画再变流畅），空闲时重新检测一次这台电脑
  useEffect(() => applyEffects(effects), [effects]);
  useEffect(() => {
    if (!info || effectsMode !== "auto") return;
    const id = requestIdleCallback(
      () => {
        const hardware = {
          cores: navigator.hardwareConcurrency || 4,
          memoryMb: info.memoryMb,
          renderer: probeRenderer(),
        };
        const low = isLowEnd(hardware);
        rememberLowEnd(low);
        setLowEnd(low);
        // 记进启动器日志：用户说「卡」时能看出是哪种电脑、自动选了什么
        void commands
          .appLog(
            "info",
            `画面效果自动判断：${low ? "流畅" : "精美"}（${hardware.cores} 线程，内存 ${hardware.memoryMb ?? "未知"} MB，显卡 ${hardware.renderer ?? "没有 WebGL"}）`,
          )
          .catch(() => undefined);
      },
      { timeout: 3000 },
    );
    return () => cancelIdleCallback(id);
  }, [effectsMode, info, setLowEnd]);

  useEffect(() => {
    if (info) applyBackdrop(info.backdrop);
  }, [info]);

  // 上次改到一半启动器被关掉的实例：后端启动时已经退回改之前，这里告诉用户一声
  useEffect(() => {
    if (!booted) return;
    commands
      .contentRecovered()
      .then(unwrap)
      .then((notes) => {
        for (const note of notes) {
          if (note.ok) {
            toast.info(
              t("safety.recovered", { instance: note.instanceName, summary: note.summary }),
              { duration: 12000 },
            );
          } else {
            toast.error(
              t("safety.recoverFailed", { instance: note.instanceName, message: note.message }),
              { duration: Number.POSITIVE_INFINITY },
            );
          }
        }
      })
      .catch((error: unknown) => reportError(error, "查看上次没做完的改动"));
  }, [booted, t]);

  // 第一帧画好后再通知后端显示窗口，避免白屏
  useEffect(() => {
    if (!booted) return;
    let second = 0;
    let details = 0;
    let updateTimer = 0;
    let cancelled = false;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => {
        commands
          .appReady()
          .then(() => {
            if (cancelled) return;
            updateTimer = window.setTimeout(() => {
              if (cancelled || useAppStore.getState().settings?.autoCheckUpdates === false) return;
              void useUpdateStore
                .getState()
                .check(true)
                .then((offer) => {
                  if (cancelled || !offer) return;
                  toast.info(t("settings.update.available", { version: offer.version }), {
                    action: {
                      label: t("settings.update.view"),
                      onClick: () => useAppStore.getState().openSettings("about"),
                    },
                  });
                });
            }, 2500);
          })
          .catch((error: unknown) => reportError(error, "显示窗口"));
        details = requestIdleCallback(
          () => {
            void loadAppDetails().catch((error: unknown) => logError(error, "读取应用环境信息"));
          },
          { timeout: 1200 },
        );
      });
    });
    return () => {
      cancelled = true;
      window.clearTimeout(updateTimer);
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
      cancelIdleCallback(details);
    };
  }, [booted, loadAppDetails, t]);

  if (!booted) return null;

  return (
    // 流畅模式和减少动效：位移、缩放类动画关掉，只留淡入淡出
    <MotionConfig reducedMotion={reduceMotion || effects === "lite" ? "always" : "user"}>
      <RouterProvider router={router} />
      <Toaster position="bottom-right" theme={theme} />
    </MotionConfig>
  );
}
