import type { TFunction } from "i18next";
import { describe, expect, it } from "vitest";
import type { JobProgress, JobSnapshot } from "./bindings";
import {
  canRetry,
  filterJobs,
  instanceOf,
  jobAmount,
  jobDiagnostics,
  jobEta,
  jobFraction,
  jobInstance,
  jobSpeed,
  overallFraction,
  playableAfter,
  recentDownloadJobs,
} from "./jobs";

const t = ((key: string, values?: Record<string, unknown>) =>
  `${key}:${values?.done}/${values?.total}`) as unknown as TFunction;

const progress = (
  done: number | null,
  total: number | null,
  unit: "bytes" | "items",
): JobProgress => ({
  done,
  total,
  unit,
  message: null,
});

describe("任务中心的小工具", () => {
  it("从键名认出实例", () => {
    expect(jobInstance("drop:验收测试:abc")).toBe("验收测试");
    expect(jobInstance("content:Cobblemon Official Modpack [NeoForge]:update")).toBe(
      "Cobblemon Official Modpack [NeoForge]",
    );
    expect(jobInstance("launch:Optimization")).toBe("Optimization");
    expect(jobInstance("modpack-import:file:整合包")).toBeNull();
    expect(playableAfter("drop:验收测试:abc")).toBe(true);
    expect(playableAfter("launch:验收测试")).toBe(false);
  });

  it("算进度和写数量", () => {
    expect(jobFraction(progress(3, 5, "items"))).toBeCloseTo(0.6);
    expect(jobFraction(progress(3, null, "items"))).toBeNull();
    expect(jobAmount(progress(3, 5, "items"), t)).toBe("tasks.items:3/5");
    expect(jobAmount(progress(1024 * 1024, 4 * 1024 * 1024, "bytes"), t)).toBe("1.0 MB / 4.0 MB");
    expect(jobAmount(progress(0, null, "bytes"), t)).toBeNull();
    const job = (p: JobProgress) => ({ progress: p }) as JobSnapshot;
    expect(
      overallFraction([
        job(progress(1, 2, "items")),
        job(progress(1, 1, "items")),
        job(progress(0, null, "bytes")),
      ]),
    ).toBeCloseTo(0.75);
    expect(overallFraction([job(progress(0, null, "bytes"))])).toBeNull();
  });
});

describe("下载中心", () => {
  it("把较早开始的进行中任务放在最近完成的任务前", () => {
    const active = job({ id: "active", state: "running", startedAt: 100 });
    const completed = job({ id: "completed", startedAt: 200 });
    expect(recentDownloadJobs([completed, active], 1)).toEqual({
      running: [active],
      visible: [active],
    });
  });

  /** 把键和参数原样拼出来，方便断言 */
  const echo = ((key: string, values?: Record<string, unknown>) =>
    values ? `${key}${JSON.stringify(values)}` : key) as unknown as TFunction;

  const job = (over: Partial<JobSnapshot>): JobSnapshot => ({
    id: "j",
    key: "content:验收测试:x",
    title: "安装 Sodium",
    state: "succeeded",
    progress: progress(10, 10, "bytes"),
    error: null,
    startedAt: 1_000,
    finishedAt: 6_000,
    source: "modrinth",
    instanceId: null,
    iconUrl: null,
    steps: [],
    speed: null,
    eta: null,
    retryable: false,
    downloadSource: null,
    fallbacks: 0,
    ...over,
  });

  it("后端记下的实例优先，没有再按键名认", () => {
    expect(instanceOf(job({ instanceId: "新实例", key: "instance-create:新实例" }))).toBe("新实例");
    expect(instanceOf(job({ key: "content:验收测试:x" }))).toBe("验收测试");
    expect(instanceOf(job({ key: "java:21" }))).toBeNull();
  });

  it("只有失败或取消、而且记了怎么重来的能重试", () => {
    expect(canRetry(job({ state: "failed", retryable: true }))).toBe(true);
    expect(canRetry(job({ state: "cancelled", retryable: true }))).toBe(true);
    expect(canRetry(job({ state: "interrupted", retryable: true })), "启动器关掉时没做完的").toBe(
      true,
    );
    expect(canRetry(job({ state: "failed", retryable: false }))).toBe(false);
    expect(canRetry(job({ state: "succeeded", retryable: true }))).toBe(false);
  });

  it("速度和剩余时间", () => {
    expect(jobSpeed(job({ speed: null }), echo)).toBeNull();
    expect(jobSpeed(job({ speed: 2.5 * 1024 * 1024 }), echo)).toBe(
      'downloads.speed{"size":"2.5 MB"}',
    );
    expect(jobSpeed(job({ speed: 3.25, progress: progress(1, 9, "items") }), echo)).toBe(
      'downloads.itemSpeed{"n":"3.3"}',
    );
    expect(jobEta(job({ eta: null }), echo)).toBeNull();
    expect(jobEta(job({ eta: 3 }), echo)).toBe("downloads.eta.almost");
    expect(jobEta(job({ eta: 21 }), echo)).toBe('downloads.eta.seconds{"n":25}');
    expect(jobEta(job({ eta: 150 }), echo)).toBe('downloads.eta.minutes{"n":3}');
    expect(jobEta(job({ eta: 3900 }), echo)).toBe('downloads.eta.hours{"h":1,"m":5}');
  });

  it("按关键词、来源、状态筛", () => {
    const list = [
      job({ id: "a", title: "安装 Sodium", source: "modrinth" }),
      job({
        id: "b",
        title: "安装整合包 FO",
        key: "modpack-import:curseforge:p",
        state: "failed",
        error: "服务器返回 503",
        source: "curseForge",
      }),
      job({ id: "c", title: "下载 Java 21", source: "java", key: "java:21" }),
      job({
        id: "d",
        title: "新建实例 生存",
        source: "game",
        key: "instance-create:生存",
        instanceId: "生存",
        steps: [{ label: "补全游戏文件", at: 2_000 }],
      }),
    ];
    const ids = (filter: Parameters<typeof filterJobs>[1]) =>
      filterJobs(list, filter, (id) => (id === "验收测试" ? "我的验收实例" : undefined)).map(
        (j) => j.id,
      );
    const all = { query: "", source: "all", state: "all" } as const;
    expect(ids(all)).toEqual(["a", "b", "c", "d"]);
    expect(ids({ ...all, source: "java" })).toEqual(["c"]);
    expect(ids({ ...all, state: "failed" })).toEqual(["b"]);
    expect(ids({ ...all, query: "503" }), "失败原因也搜").toEqual(["b"]);
    expect(ids({ ...all, query: "我的验收" }), "实例名也搜").toEqual(["a"]);
    expect(ids({ ...all, query: "补全" }), "做过的步骤也搜").toEqual(["d"]);
    expect(ids({ ...all, query: "SODIUM" }), "不分大小写").toEqual(["a"]);
  });

  it("诊断信息带上任务、每一步和下载设置", () => {
    const text = jobDiagnostics(
      job({
        state: "failed",
        error: "网络断开",
        steps: [
          { label: "下载整合包", at: 1_000 },
          { label: "下载整合包文件", at: 3_000 },
        ],
      }),
      { appVersion: "0.3.0", instanceName: "我的验收实例", settings: null, speed: null },
      echo,
    );
    const lines = text.split("\n");
    expect(lines[0]).toBe('downloads.diag.title{"title":"安装 Sodium"}');
    expect(text).toContain("网络断开");
    expect(text).toContain("我的验收实例");
    expect(text).toContain("下载整合包文件");
    expect(text).toContain('downloads.diag.version{"version":"0.3.0"}');
    const mirrored = jobDiagnostics(
      job({ downloadSource: "mirror", fallbacks: 3 }),
      { appVersion: null, instanceName: null, settings: null, speed: null },
      echo,
    );
    expect(mirrored).toContain('downloads.diag.via{"source":"downloads.viaSource.mirror","n":3}');
  });
});
