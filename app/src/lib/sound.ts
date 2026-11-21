// 微光轻音效：纯 Web Audio 合成的极克制柔音，零第三方依赖、零网络、默认关闭。
// 后续若作者提供原声采样文件，可在此模块内无缝替换为缓冲播放，调用方不用改。

const SOUND_KEY = "mcl.soundEffects";

export type SoundKind = "launch" | "complete" | "rollback" | "toggle";

const listeners = new Set<() => void>();
let cached: boolean | null = null;
let audioCtx: AudioContext | null = null;

function readPreference(): boolean {
  try {
    return localStorage.getItem(SOUND_KEY) === "1";
  } catch {
    return false;
  }
}

export function getSoundEnabled(): boolean {
  if (cached === null) cached = readPreference();
  return cached;
}

export function setSoundEnabled(enabled: boolean): void {
  cached = enabled;
  try {
    localStorage.setItem(SOUND_KEY, enabled ? "1" : "0");
  } catch {
    // 存储不可用时仅在本次会话生效
  }
  for (const listener of listeners) listener();
  if (enabled) playLumiSound("toggle");
}

export function subscribeSoundEnabled(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

interface Tone {
  freq: number;
  start: number;
  duration: number;
  gain: number;
  type?: OscillatorType;
}

const VOICES: Record<SoundKind, Tone[]> = {
  // 点击「继续 / 开始游戏」：柔和上扬的五度玻璃共鸣
  launch: [
    { freq: 523.25, start: 0, duration: 0.24, gain: 0.038, type: "sine" },
    { freq: 783.99, start: 0.055, duration: 0.28, gain: 0.032, type: "sine" },
    { freq: 1046.5, start: 0.09, duration: 0.24, gain: 0.016, type: "triangle" },
  ],
  // 下载或安装完成：清澈微光提示
  complete: [
    { freq: 659.25, start: 0, duration: 0.18, gain: 0.032, type: "sine" },
    { freq: 987.77, start: 0.06, duration: 0.24, gain: 0.028, type: "sine" },
  ],
  // 崩溃后一键回滚成功：温和回落再点亮
  rollback: [
    { freq: 783.99, start: 0, duration: 0.15, gain: 0.03, type: "sine" },
    { freq: 587.33, start: 0.06, duration: 0.16, gain: 0.03, type: "sine" },
    { freq: 880, start: 0.13, duration: 0.26, gain: 0.032, type: "sine" },
  ],
  // 设置里开启轻音效时的试听轻音
  toggle: [{ freq: 783.99, start: 0, duration: 0.16, gain: 0.028, type: "sine" }],
};

function getAudioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  const Ctor =
    window.AudioContext ??
    (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!Ctor) return null;
  if (!audioCtx) audioCtx = new Ctor();
  return audioCtx;
}

/** 播放一枚极轻柔的微光交互音效。未开启或运行环境不支持音频时静默跳过。 */
export function playLumiSound(kind: SoundKind): void {
  if (!getSoundEnabled()) return;
  try {
    const ctx = getAudioContext();
    if (!ctx) return;
    if (ctx.state === "suspended") {
      void ctx.resume().catch(() => {
        // 未触发用户手势前浏览器会拦截自动恢复，静默忽略
      });
    }
    const now = ctx.currentTime;
    for (const tone of VOICES[kind]) {
      const osc = ctx.createOscillator();
      const amp = ctx.createGain();
      osc.type = tone.type ?? "sine";
      osc.frequency.setValueAtTime(tone.freq, now + tone.start);

      amp.gain.setValueAtTime(0.0001, now + tone.start);
      amp.gain.exponentialRampToValueAtTime(tone.gain, now + tone.start + 0.018);
      amp.gain.exponentialRampToValueAtTime(0.0001, now + tone.start + tone.duration);

      osc.connect(amp);
      amp.connect(ctx.destination);
      osc.start(now + tone.start);
      osc.stop(now + tone.start + tone.duration + 0.02);
    }
  } catch {
    // 音频设备异常不打扰界面主流程
  }
}
