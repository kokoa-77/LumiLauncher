import { ChevronDown, Copy, HardDrive, ShieldCheck, Wifi } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { unwrap } from "@/lib/api";
import {
  type ClaimFolderInfo,
  type ConcurrencyMode,
  commands,
  type DiskWriteMode,
  type DnsProbe,
  type EndpointProbe,
  type MeteredPolicy,
  type Platform,
  type ProxyMode,
  type ProxyScheme,
  type ProxyTestResult,
  type SegmentMode,
  type Settings,
  type TransferMetrics,
} from "@/lib/bindings";
import { logError, reportError } from "@/lib/report";
import { Button } from "../ui/Button";
import { TextInput } from "../ui/Field";
import { Truncated } from "../ui/HoverCard";
import { Segmented } from "../ui/Segmented";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";
import { Group, Row, type UpdateSettings } from "./parts";

const CONCURRENCY_CHOICES = ["8", "16", "32", "64"] as const;
const SEGMENT_CHOICES = ["4", "8", "16", "24", "32"] as const;
const DISK_WRITER_CHOICES = ["2", "4", "8", "12", "16", "24"] as const;
function isPublicHttps443Url(raw: string): boolean {
  const trimmed = raw.trim();
  if (!trimmed || trimmed.length > 512 || /\s/.test(trimmed)) return false;
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return false;
  }
  if (url.protocol !== "https:") return false;
  if (url.username || url.password || url.search || url.hash) return false;
  if (url.port && url.port !== "443") return false;
  const host = url.hostname.replace(/^\[|\]$/g, "").toLowerCase();
  if (!host || host === "localhost" || host.endsWith(".localhost")) return false;
  const ipv4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
  if (ipv4) {
    const octets = ipv4.slice(1).map((n) => Number.parseInt(n, 10));
    if (octets.some((o) => o > 255)) return false;
    const [a, b] = octets as [number, number, number, number];
    if (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 100 && b >= 64 && b <= 127) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 198 && (b === 18 || b === 19)) ||
      a >= 224
    ) {
      return false;
    }
  } else if (host.includes(":")) {
    if (
      host === "::" ||
      host === "::1" ||
      host.startsWith("fe80:") ||
      host.startsWith("fc") ||
      host.startsWith("fd") ||
      host.startsWith("::ffff:")
    ) {
      return false;
    }
  } else if (
    !host.includes(".") ||
    host.startsWith(".") ||
    host.endsWith(".") ||
    host.includes("..") ||
    host.endsWith(".local") ||
    host.endsWith(".lan") ||
    host.endsWith(".internal") ||
    host.endsWith(".arpa") ||
    host.endsWith(".home.arpa")
  ) {
    return false;
  }
  return true;
}

const SPEED_PRESETS = [
  { kib: 0, labelKey: "settings.speedUnlimited" },
  { kib: 5 * 1024, label: "5 MB/s" },
  { kib: 10 * 1024, label: "10 MB/s" },
  { kib: 20 * 1024, label: "20 MB/s" },
] as const;

const ADVANCED_IDS = new Set([
  "concurrency",
  "segments",
  "diskWriters",
  "speedLimit",
  "localReuse",
  "doh",
]);

function parsePort(text: string, fallback: number): number {
  const parsed = Number.parseInt(text.trim(), 10);
  if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) return fallback;
  return parsed;
}

/**
 * 设置 →「内容与下载」v2.0：
 * - 实时网络与存储状态条
 * - 内容偏好：首选平台、自动装前置
 * - 下载与网络：代理（含「测试连接」）、计费网络保护策略
 * - 折叠高级传输调优：并发、分段、并发写入、热限速、本地零字节复用、DoH 与自定义 DoH
 * - 6 端点深度网络诊断（含 NeoForge 主站与官方 CDN 对比）
 * - 浏览器补件
 */
export function DownloadSettings({
  settings,
  update,
  focusSettingId,
  onFocusConsumed,
}: {
  settings: Settings;
  update: UpdateSettings;
  focusSettingId?: string | null;
  onFocusConsumed?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const isZh = i18n.language.toLowerCase().startsWith("zh");
  const download = settings.download;

  const [runtime, setRuntime] = useState<TransferMetrics | null>(null);
  const [dns, setDns] = useState<DnsProbe[] | null>(null);
  const [dnsBusy, setDnsBusy] = useState(false);
  const [probes, setProbes] = useState<EndpointProbe[] | null>(null);
  const [probesBusy, setProbesBusy] = useState(false);
  const [folder, setFolder] = useState<ClaimFolderInfo | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  const [proxyModeDraft, setProxyModeDraft] = useState<ProxyMode>(download.proxy.mode);
  const [proxyScheme, setProxyScheme] = useState<ProxyScheme>(download.proxy.scheme);
  const [proxyHost, setProxyHost] = useState(download.proxy.host);
  const [proxyPort, setProxyPort] = useState(String(download.proxy.port || 7890));
  const [proxyTesting, setProxyTesting] = useState(false);
  const [proxyTest, setProxyTest] = useState<ProxyTestResult | null>(null);

  const speedLimit = download.speedLimitKibPerSec ?? 0;
  const meteredPolicy = download.meteredPolicy ?? "warn";
  const meteredSpeedLimit = download.meteredSpeedLimitKibPerSec ?? 2048;
  const concurrencyMode = download.concurrencyMode ?? "auto";
  const segmentMode = download.segmentMode ?? "auto";
  const maxSegments = download.maxSegmentsPerFile ?? 16;
  const diskWriteMode = download.diskWriteMode ?? "auto";
  const maxDiskWriters = download.maxDiskWriters ?? 12;
  const localReuse = download.localReuseEnabled ?? true;

  const [customDohDraft, setCustomDohDraft] = useState(download.customDohUrl ?? "");
  const [customConcurrencyText, setCustomConcurrencyText] = useState(
    String(download.maxConcurrency),
  );
  const [customSpeedMb, setCustomSpeedMb] = useState(
    speedLimit > 0 ? String(Math.round((speedLimit / 1024) * 10) / 10) : "",
  );

  useEffect(() => {
    setProxyModeDraft(download.proxy.mode);
    setProxyScheme(download.proxy.scheme);
    setProxyHost(download.proxy.host);
    setProxyPort(String(download.proxy.port || 7890));
  }, [download.proxy.mode, download.proxy.scheme, download.proxy.host, download.proxy.port]);

  useEffect(() => {
    setCustomDohDraft(download.customDohUrl ?? "");
  }, [download.customDohUrl]);

  useEffect(() => {
    setCustomConcurrencyText(String(download.maxConcurrency));
  }, [download.maxConcurrency]);

  useEffect(() => {
    commands
      .claimFolderInfo()
      .then(setFolder)
      .catch((error: unknown) => logError(error, "读取补件文件夹"));
  }, []);

  useEffect(() => {
    commands
      .netRuntimeStatus()
      .then(setRuntime)
      .catch((error: unknown) => logError(error, "读取网络状态"));
  }, [download]);

  useEffect(() => {
    if (!focusSettingId) return;
    if (ADVANCED_IDS.has(focusSettingId)) {
      setAdvancedOpen(true);
    }
    const timer = window.setTimeout(() => {
      const el = document.getElementById(`setting-row-${focusSettingId}`);
      el?.scrollIntoView({ behavior: "smooth", block: "center" });
      onFocusConsumed?.();
    }, 60);
    return () => window.clearTimeout(timer);
  }, [focusSettingId, onFocusConsumed]);

  const set = (patch: Partial<Settings["download"]>) =>
    update((s) => ({ ...s, download: { ...s.download, ...patch } }));

  const onProxyMode = (mode: ProxyMode) => {
    setProxyModeDraft(mode);
    setProxyTest(null);
    if (mode !== "custom") {
      set({ proxy: { ...download.proxy, mode } });
      toast.success(t("settings.proxySaved"));
    }
  };

  const applyCustomProxy = () => {
    const host = proxyHost.trim();
    if (!host) {
      toast.error(t("settings.proxyHostRequired"));
      return;
    }
    const port = parsePort(proxyPort, download.proxy.port || 7890);
    setProxyPort(String(port));
    set({
      proxy: {
        mode: "custom",
        scheme: proxyScheme,
        host,
        port,
      },
    });
    toast.success(t("settings.proxySaved"));
  };

  const testProxy = async () => {
    setProxyTesting(true);
    setProxyTest(null);
    try {
      const port = parsePort(proxyPort, download.proxy.port || 7890);
      const targetProxy =
        proxyModeDraft === "custom"
          ? { mode: "custom" as ProxyMode, scheme: proxyScheme, host: proxyHost.trim(), port }
          : { ...download.proxy, mode: proxyModeDraft };
      const res = unwrap(await commands.netTestProxy(targetProxy));
      setProxyTest(res);
    } catch (error) {
      reportError(error, "测试代理连接");
    } finally {
      setProxyTesting(false);
    }
  };

  const applyCustomDoh = () => {
    const trimmed = customDohDraft.trim();
    if (!trimmed) {
      set({ customDohUrl: null });
      return;
    }
    if (!isPublicHttps443Url(trimmed)) {
      toast.error(t("settings.customDohInvalid"));
      return;
    }
    set({ customDohUrl: trimmed });
    toast.success(t("settings.customDohSaved"));
  };

  const applySpeedCustom = () => {
    const mb = Number.parseFloat(customSpeedMb.trim());
    if (!Number.isFinite(mb) || mb <= 0) {
      set({ speedLimitKibPerSec: 0 });
      setCustomSpeedMb("");
      return;
    }
    const kib = Math.min(1_048_576, Math.max(128, Math.round(mb * 1024)));
    set({ speedLimitKibPerSec: kib });
  };

  const testDns = async () => {
    setDnsBusy(true);
    try {
      setDns(unwrap(await commands.netDohTest()));
    } catch (error) {
      reportError(error, "测试 DoH");
    } finally {
      setDnsBusy(false);
    }
  };

  const runDiagnostics = async () => {
    setProbesBusy(true);
    try {
      setProbes(unwrap(await commands.netDiagnostics()));
    } catch (error) {
      reportError(error, "网络连通性检测");
    } finally {
      setProbesBusy(false);
    }
  };

  const copyDiagnostics = async () => {
    if (!probes) return;
    const lines = [
      `Proxy: ${download.proxy.mode}${download.proxy.mode === "custom" ? ` (${download.proxy.scheme})` : ""} | DoH: ${download.dohEnabled ? "on" : "off"} | Concurrency: ${download.concurrencyMode}(${download.maxConcurrency}) | Segment: ${download.segmentMode} | Disk: ${runtime?.diskKind ?? "unknown"} (${download.diskWriteMode}:${download.maxDiskWriters})`,
      ...probes.map((probe) => {
        const status = t(`settings.netDiagStatus.${probe.status}`);
        const latency = probe.latencyMs != null ? `${Math.round(probe.latencyMs)} ms` : "-";
        const ver = probe.httpVersion ? ` · ${probe.httpVersion}` : "";
        const extra = probe.httpStatus != null ? `HTTP ${probe.httpStatus}` : (probe.error ?? "");
        return `- ${t(`settings.netDiagEndpoint.${probe.id}`)} (${probe.host}): ${status} · ${latency}${ver}${extra ? ` · ${extra}` : ""}`;
      }),
    ];
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      toast.success(t("settings.netDiagCopied"));
    } catch (error) {
      reportError(error, "复制网络诊断结果");
    }
  };

  const chooseFolder = async () => {
    try {
      const chosen = unwrap(await commands.claimFolderChoose());
      if (chosen) setFolder(chosen);
    } catch (error) {
      reportError(error, "选择补件文件夹");
    }
  };

  const resetFolder = async () => {
    try {
      setFolder(unwrap(await commands.claimFolderReset()));
    } catch (error) {
      reportError(error, "改回系统的下载文件夹");
    }
  };

  const concurrencyStr = String(download.maxConcurrency);
  const concurrencyOptions = CONCURRENCY_CHOICES.includes(
    concurrencyStr as (typeof CONCURRENCY_CHOICES)[number],
  )
    ? CONCURRENCY_CHOICES
    : ([...CONCURRENCY_CHOICES, concurrencyStr] as string[]);

  const segmentLabel =
    download.segmentMode === "auto"
      ? t("settings.segmentSummaryAuto", { n: download.maxSegmentsPerFile ?? 16 })
      : download.segmentMode === "aggressive"
        ? t("settings.segmentSummaryAggressive", { n: download.maxSegmentsPerFile })
        : t("settings.segmentSummaryOff");
  const concLabel =
    download.concurrencyMode === "auto"
      ? t("settings.concurrencySummaryAuto", { n: runtime?.maxConnections ?? 64 })
      : String(download.maxConcurrency);
  const diskLabel =
    download.diskWriteMode === "auto"
      ? `${(runtime?.diskKind ?? "unknown").toUpperCase()}(${runtime?.diskWritersLimit ?? 12})`
      : String(download.maxDiskWriters);

  return (
    <>
      {runtime && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-panel border border-line bg-surface-2 px-4 py-2.5 text-xs">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-fg-muted tabular-nums">
            <span className="inline-flex items-center gap-1.5 font-medium text-fg">
              <span className="size-2 rounded-full bg-accent" aria-hidden />
              {t("settings.statusRoute", { label: runtime.routeLabel })}
            </span>
            <span>
              ·{" "}
              {download.dohEnabled
                ? (runtime.dohSummary ?? t("settings.statusDohReady"))
                : t("settings.statusDohOff")}
            </span>
            <span className="inline-flex items-center gap-1">
              · <HardDrive className="size-3.5" aria-hidden />
              {runtime.diskKind.toUpperCase()} ({runtime.diskWritersLimit})
            </span>
            {runtime.metered && (
              <span className="inline-flex items-center gap-1 rounded-tag bg-warning/15 px-1.5 py-0.5 text-warning">
                <Wifi className="size-3" aria-hidden />
                {t("settings.statusMetered")}
              </span>
            )}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              document
                .getElementById("setting-row-netdiag")
                ?.scrollIntoView({ behavior: "smooth", block: "center" });
              void runDiagnostics();
            }}
          >
            {t("settings.statusQuickDiag")}
          </Button>
        </div>
      )}

      <Group title={t("settings.contentGroup")}>
        <div id="setting-row-platform">
          <Row
            label={t("settings.preferredPlatform")}
            hint={t("settings.preferredPlatformHint")}
            control={
              <Segmented<Platform>
                label={t("settings.preferredPlatform")}
                value={download.preferredPlatform}
                options={[
                  { value: "modrinth", label: "Modrinth" },
                  { value: "curseForge", label: "CurseForge" },
                ]}
                onChange={(preferredPlatform) => set({ preferredPlatform })}
              />
            }
          />
        </div>
        <div id="setting-row-autoDeps">
          <Row
            label={t("settings.autoDependencies")}
            hint={t("settings.autoDependenciesHint")}
            control={
              <Switch
                label={t("settings.autoDependencies")}
                checked={download.autoDependencies}
                onChange={(autoDependencies) => set({ autoDependencies })}
              />
            }
          />
        </div>
      </Group>

      <Group title={t("settings.downloadGroup")}>
        <div id="setting-row-proxy">
          <Row
            label={t("settings.proxy")}
            hint={t("settings.proxyHint")}
            control={
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={proxyTesting}
                  onClick={() => void testProxy()}
                >
                  {proxyTesting && <LightSpinner className="size-3.5" />}
                  {t("settings.proxyTest")}
                </Button>
                <Segmented<ProxyMode>
                  label={t("settings.proxy")}
                  value={proxyModeDraft}
                  options={[
                    { value: "system", label: t("settings.proxyMode.system") },
                    { value: "direct", label: t("settings.proxyMode.direct") },
                    { value: "custom", label: t("settings.proxyMode.custom") },
                  ]}
                  onChange={onProxyMode}
                />
              </div>
            }
          />
          {proxyTest && (
            <p
              className={`px-4 pb-2 text-xs tabular-nums ${
                proxyTest.ok ? "text-accent" : "text-danger"
              }`}
            >
              {proxyTest.ok
                ? t("settings.proxyTestOk", {
                    ms: proxyTest.latencyMs ?? 0,
                    status: proxyTest.httpStatus ?? 200,
                  })
                : t("settings.proxyTestFail", { error: proxyTest.error ?? "-" })}
            </p>
          )}
          {proxyModeDraft === "custom" && (
            <div className="flex flex-wrap items-center gap-2 px-4 py-3">
              <div className="w-28 shrink-0">
                <Select
                  label={t("settings.proxyScheme")}
                  value={proxyScheme}
                  onChange={(value) => setProxyScheme(value as ProxyScheme)}
                  options={[
                    { value: "http", label: "HTTP" },
                    { value: "socks5", label: "SOCKS5" },
                  ]}
                />
              </div>
              <div className="min-w-40 flex-1">
                <TextInput
                  value={proxyHost}
                  onChange={(event) => setProxyHost(event.target.value)}
                  placeholder={t("settings.proxyHostPlaceholder")}
                  aria-label={t("settings.proxyHost")}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") applyCustomProxy();
                  }}
                />
              </div>
              <div className="w-24 shrink-0">
                <TextInput
                  value={proxyPort}
                  onChange={(event) => setProxyPort(event.target.value)}
                  inputMode="numeric"
                  className="px-2 text-center tabular-nums"
                  aria-label={t("settings.proxyPort")}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") applyCustomProxy();
                  }}
                />
              </div>
              <Button variant="secondary" onClick={applyCustomProxy}>
                {t("settings.proxyApply")}
              </Button>
            </div>
          )}
        </div>

        <div id="setting-row-metered">
          <Row
            label={t("settings.meteredPolicy")}
            hint={t("settings.meteredPolicyHint")}
            control={
              <Segmented<MeteredPolicy>
                label={t("settings.meteredPolicy")}
                value={meteredPolicy}
                options={[
                  { value: "warn", label: t("settings.meteredOption.warn") },
                  { value: "throttle", label: t("settings.meteredOption.throttle") },
                  { value: "allow", label: t("settings.meteredOption.allow") },
                ]}
                onChange={(nextPolicy) => set({ meteredPolicy: nextPolicy })}
              />
            }
          />
          {meteredPolicy === "throttle" && (
            <div className="flex flex-wrap items-center justify-between gap-2 px-4 pb-3 text-xs">
              <span className="text-fg-muted">{t("settings.meteredSpeedCap")}</span>
              <Segmented<string>
                label={t("settings.meteredSpeedCap")}
                value={String(meteredSpeedLimit)}
                options={[
                  { value: "1024", label: "1 MB/s" },
                  { value: "2048", label: "2 MB/s" },
                  { value: "5120", label: "5 MB/s" },
                ]}
                onChange={(v) => set({ meteredSpeedLimitKibPerSec: Number(v) })}
              />
            </div>
          )}
        </div>
      </Group>

      <Group title={t("settings.advancedTransportGroup")}>
        <Row
          label={t("settings.advancedTransportToggle")}
          hint={t("settings.advancedTransportSummary", {
            conc: concLabel,
            seg: segmentLabel,
            disk: diskLabel,
          })}
          control={
            <Button
              variant="secondary"
              size="sm"
              aria-expanded={advancedOpen}
              onClick={() => setAdvancedOpen((v) => !v)}
            >
              <ChevronDown
                className={`size-3.5 transition-transform ${advancedOpen ? "rotate-180" : ""}`}
                aria-hidden
              />
              {advancedOpen ? t("settings.advanced.collapse") : t("settings.advanced.expand")}
            </Button>
          }
        />

        {advancedOpen && (
          <>
            <div id="setting-row-concurrency">
              <Row
                label={t("settings.maxConcurrency")}
                hint={t("settings.maxConcurrencyHint")}
                control={
                  <Segmented<ConcurrencyMode>
                    label={t("settings.maxConcurrency")}
                    value={concurrencyMode}
                    options={[
                      { value: "auto", label: t("settings.modeAuto") },
                      { value: "manual", label: t("settings.modeManual") },
                    ]}
                    onChange={(nextMode) => set({ concurrencyMode: nextMode })}
                  />
                }
              />
              {concurrencyMode === "manual" && (
                <div className="flex flex-col gap-2 px-4 pb-3">
                  <div className="flex flex-wrap items-center justify-end gap-2">
                    <Segmented<string>
                      label={t("settings.maxConcurrency")}
                      value={concurrencyStr}
                      options={concurrencyOptions.map((value) => ({ value, label: value }))}
                      onChange={(value) => set({ maxConcurrency: Number(value) })}
                    />
                    <div className="w-20">
                      <TextInput
                        value={customConcurrencyText}
                        onChange={(e) => setCustomConcurrencyText(e.target.value)}
                        onBlur={() => {
                          const n = Number.parseInt(customConcurrencyText.trim(), 10);
                          if (Number.isFinite(n)) {
                            set({ maxConcurrency: Math.min(128, Math.max(1, n)) });
                          }
                        }}
                        inputMode="numeric"
                        className="px-2 text-center tabular-nums"
                        aria-label={t("settings.maxConcurrency")}
                      />
                    </div>
                  </div>
                  {download.maxConcurrency > 64 && (
                    <p className="text-right text-warning text-xs">
                      {t("settings.highConcurrencyWarning")}
                    </p>
                  )}
                </div>
              )}
            </div>

            <div id="setting-row-segments">
              <Row
                label={t("settings.segmentMode")}
                hint={t("settings.segmentModeHint")}
                control={
                  <Segmented<SegmentMode>
                    label={t("settings.segmentMode")}
                    value={segmentMode}
                    options={[
                      { value: "auto", label: t("settings.segmentOption.auto") },
                      { value: "aggressive", label: t("settings.segmentOption.aggressive") },
                      { value: "off", label: t("settings.segmentOption.off") },
                    ]}
                    onChange={(nextSegmentMode) => set({ segmentMode: nextSegmentMode })}
                  />
                }
              />
              {segmentMode === "aggressive" && (
                <div className="flex flex-col gap-1.5 px-4 pb-3">
                  <div className="flex items-center justify-between gap-2 text-xs">
                    <span className="text-fg-muted">{t("settings.maxSegmentsPerFile")}</span>
                    <Segmented<string>
                      label={t("settings.maxSegmentsPerFile")}
                      value={String(maxSegments)}
                      options={SEGMENT_CHOICES.map((v) => ({ value: v, label: v }))}
                      onChange={(v) => set({ maxSegmentsPerFile: Number(v) })}
                    />
                  </div>
                  <p className="text-warning text-xs">{t("settings.aggressiveSegmentWarning")}</p>
                </div>
              )}
            </div>

            <div id="setting-row-diskWriters">
              <Row
                label={t("settings.diskWriteMode")}
                hint={t("settings.diskWriteModeHint")}
                control={
                  <Segmented<DiskWriteMode>
                    label={t("settings.diskWriteMode")}
                    value={diskWriteMode}
                    options={[
                      { value: "auto", label: t("settings.modeAuto") },
                      { value: "manual", label: t("settings.modeManual") },
                    ]}
                    onChange={(nextDiskMode) => set({ diskWriteMode: nextDiskMode })}
                  />
                }
              />
              {diskWriteMode === "manual" && (
                <div className="flex justify-end px-4 pb-3">
                  <Segmented<string>
                    label={t("settings.maxDiskWriters")}
                    value={String(maxDiskWriters)}
                    options={DISK_WRITER_CHOICES.map((v) => ({ value: v, label: v }))}
                    onChange={(v) => set({ maxDiskWriters: Number(v) })}
                  />
                </div>
              )}
            </div>

            <div id="setting-row-speedLimit">
              <Row
                label={t("settings.speedLimit")}
                hint={t("settings.speedLimitHint")}
                control={
                  <div className="flex flex-wrap items-center gap-2">
                    <Segmented<string>
                      label={t("settings.speedLimit")}
                      value={String(speedLimit)}
                      options={SPEED_PRESETS.map((p) => ({
                        value: String(p.kib),
                        label: "labelKey" in p ? t(p.labelKey) : p.label,
                      }))}
                      onChange={(v) => {
                        const kib = Number(v);
                        set({ speedLimitKibPerSec: kib });
                        setCustomSpeedMb(kib > 0 ? String(Math.round(kib / 1024)) : "");
                      }}
                    />
                    <div className="w-24">
                      <TextInput
                        value={customSpeedMb}
                        onChange={(e) => setCustomSpeedMb(e.target.value)}
                        onBlur={applySpeedCustom}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") applySpeedCustom();
                        }}
                        placeholder="MB/s"
                        aria-label={t("settings.speedLimit")}
                        className="px-2 text-center tabular-nums"
                      />
                    </div>
                  </div>
                }
              />
            </div>

            <div id="setting-row-localReuse">
              <Row
                label={t("settings.localReuse")}
                hint={t("settings.localReuseHint")}
                control={
                  <div className="flex items-center gap-2">
                    <ShieldCheck className="size-4 text-accent" aria-hidden />
                    <Switch
                      label={t("settings.localReuse")}
                      checked={localReuse}
                      onChange={(localReuseEnabled) => set({ localReuseEnabled })}
                    />
                  </div>
                }
              />
            </div>

            <div id="setting-row-doh">
              <Row
                label={t("settings.doh")}
                hint={t("settings.dohHonestHint")}
                control={
                  <div className="flex items-center gap-3">
                    <Button variant="secondary" disabled={dnsBusy} onClick={() => void testDns()}>
                      {dnsBusy && <LightSpinner className="size-4" />}
                      {dnsBusy ? t("settings.dohTesting") : t("settings.dohTest")}
                    </Button>
                    <Switch
                      label={t("settings.doh")}
                      checked={download.dohEnabled}
                      onChange={(dohEnabled) => set({ dohEnabled })}
                    />
                  </div>
                }
              />
              {download.dohEnabled && (
                <div className="flex flex-wrap items-center gap-2 px-4 pb-3">
                  <div className="min-w-52 flex-1">
                    <TextInput
                      value={customDohDraft}
                      onChange={(e) => setCustomDohDraft(e.target.value)}
                      placeholder={t("settings.customDohPlaceholder")}
                      aria-label={t("settings.customDohLabel")}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") applyCustomDoh();
                      }}
                    />
                  </div>
                  <Button variant="secondary" size="sm" onClick={applyCustomDoh}>
                    {t("settings.proxyApply")}
                  </Button>
                </div>
              )}
              {dns && (
                <ul className="flex flex-col gap-1 px-4 py-3 text-xs">
                  {dns.map((probe) => (
                    <li key={probe.name} className="flex gap-3">
                      <span className="w-36 shrink-0 text-fg-muted">{probe.name}</span>
                      {probe.ms != null ? (
                        <span className="min-w-0 truncate tabular-nums">
                          {t("settings.dohOk", {
                            ms: Math.round(probe.ms),
                            address: probe.addresses.join(isZh ? "、" : ", "),
                          })}
                        </span>
                      ) : (
                        <span className="text-danger">{probe.error}</span>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </>
        )}
      </Group>

      <Group title={t("settings.netDiagGroup")}>
        <div id="setting-row-netdiag">
          <Row
            label={t("settings.netDiagLabel")}
            hint={t("settings.netDiagHint")}
            control={
              <div className="flex items-center gap-2">
                {probes && (
                  <Button variant="ghost" onClick={() => void copyDiagnostics()}>
                    <Copy className="size-3.5" aria-hidden />
                    {t("settings.netDiagCopy")}
                  </Button>
                )}
                <Button
                  variant="secondary"
                  disabled={probesBusy}
                  onClick={() => void runDiagnostics()}
                >
                  {probesBusy && <LightSpinner className="size-4" />}
                  {probesBusy ? t("settings.netDiagRunning") : t("settings.netDiagRun")}
                </Button>
              </div>
            }
          />
          {probes && (
            <ul className="flex flex-col gap-1.5 px-4 py-3 text-xs">
              {probes.map((probe) => {
                const dotClass =
                  probe.status === "ok"
                    ? "bg-accent"
                    : probe.status === "restricted"
                      ? "bg-warning"
                      : "bg-danger";
                const textClass =
                  probe.status === "ok"
                    ? "text-fg"
                    : probe.status === "restricted"
                      ? "text-warning"
                      : "text-danger";
                return (
                  <li key={probe.id} className="flex items-center gap-2.5">
                    <span className={`size-2 shrink-0 rounded-tag ${dotClass}`} aria-hidden />
                    <span className="w-36 shrink-0 font-medium text-fg">
                      {t(`settings.netDiagEndpoint.${probe.id}`)}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-fg-muted">
                      {probe.host}
                    </span>
                    <span className={`shrink-0 tabular-nums ${textClass}`}>
                      {t(`settings.netDiagStatus.${probe.status}`)}
                      {probe.latencyMs != null ? ` · ${Math.round(probe.latencyMs)} ms` : ""}
                      {probe.httpVersion ? ` · ${probe.httpVersion}` : ""}
                      {probe.status !== "ok" && probe.httpStatus != null
                        ? ` (${probe.httpStatus})`
                        : ""}
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </Group>

      <Group title={t("settings.claimGroup")}>
        <div id="setting-row-claim">
          <Row
            label={t("settings.claimFolder")}
            hint={t("settings.claimFolderHint")}
            control={
              <div className="flex min-w-0 items-center gap-2">
                <Truncated
                  text={folder?.path ?? ""}
                  className="max-w-52 select-text truncate text-fg-muted text-xs"
                >
                  {folder?.custom ? folder.path : t("settings.claimFolderSystem")}
                </Truncated>
                {folder?.custom && (
                  <Button variant="ghost" onClick={() => void resetFolder()}>
                    {t("settings.claimFolderReset")}
                  </Button>
                )}
                <Button variant="secondary" onClick={() => void chooseFolder()}>
                  {t("settings.claimFolderChange")}
                </Button>
              </div>
            }
          />
          <Row
            label={t("settings.moveClaimed")}
            hint={t("settings.moveClaimedHint")}
            control={
              <Switch
                label={t("settings.moveClaimed")}
                checked={download.moveClaimedFiles ?? false}
                onChange={(moveClaimedFiles) => set({ moveClaimedFiles })}
              />
            }
          />
        </div>
      </Group>
    </>
  );
}
