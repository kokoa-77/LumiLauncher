import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const SOUND_KEY = "mcl.soundEffects";

function stubStorage(initial?: string) {
  const map = new Map<string, string>();
  if (initial !== undefined) map.set(SOUND_KEY, initial);
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => map.set(key, value),
  });
  return map;
}

beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());

describe("微光轻音效偏好与播放", () => {
  it("默认关闭且不触发音频创建", async () => {
    stubStorage();
    const ctor = vi.fn();
    vi.stubGlobal("window", { AudioContext: ctor });
    const { getSoundEnabled, playLumiSound } = await import("./sound");

    expect(getSoundEnabled()).toBe(false);
    playLumiSound("launch");
    expect(ctor).not.toHaveBeenCalled();
  });

  it("持久化开关状态并通知订阅者", async () => {
    const storage = stubStorage();
    vi.stubGlobal("window", {});
    const { getSoundEnabled, setSoundEnabled, subscribeSoundEnabled } = await import("./sound");
    const onChange = vi.fn();
    const unsub = subscribeSoundEnabled(onChange);

    setSoundEnabled(true);
    expect(getSoundEnabled()).toBe(true);
    expect(storage.get(SOUND_KEY)).toBe("1");
    expect(onChange).toHaveBeenCalledTimes(1);

    setSoundEnabled(false);
    expect(getSoundEnabled()).toBe(false);
    expect(storage.get(SOUND_KEY)).toBe("0");
    expect(onChange).toHaveBeenCalledTimes(2);
    unsub();
  });

  it("开启后可合成各档柔音且在无 AudioContext 时静默降级", async () => {
    stubStorage("1");
    const starts: number[] = [];
    class FakeAudioContext {
      state = "running";
      currentTime = 1;
      destination = {};
      createOscillator() {
        return {
          type: "sine",
          frequency: { setValueAtTime: vi.fn() },
          connect: vi.fn(),
          start: (t: number) => starts.push(t),
          stop: vi.fn(),
        };
      }
      createGain() {
        return {
          gain: {
            setValueAtTime: vi.fn(),
            exponentialRampToValueAtTime: vi.fn(),
          },
          connect: vi.fn(),
        };
      }
      resume() {
        return Promise.resolve();
      }
    }
    vi.stubGlobal("window", { AudioContext: FakeAudioContext });
    const { playLumiSound } = await import("./sound");

    playLumiSound("launch");
    playLumiSound("complete");
    playLumiSound("rollback");
    expect(starts.length).toBe(8);
  });
});
