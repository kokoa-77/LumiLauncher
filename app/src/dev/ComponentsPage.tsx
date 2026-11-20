// 组件展示页：只在开发版里有（router.tsx 里按 import.meta.env.DEV 挂上，正式包里整个文件都不会打进去）。
// 一页摆出所有组件的所有状态，换主题、换主色、换圆角、切画面效果都在这里看。
// 这里的文字仅用于本地组件预览，不走生产 i18n。
import {
  Download,
  MoreHorizontal,
  Package,
  Play,
  RefreshCw,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import { MotionConfig } from "motion/react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { MemoryBar } from "@/components/MemoryBar";
import { Stagger, StaggerItem } from "@/components/Stagger";
import { Button } from "@/components/ui/Button";
import { CtaButton } from "@/components/ui/CtaButton";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { Field, TextInput } from "@/components/ui/Field";
import { IconButton } from "@/components/ui/IconButton";
import { Menu } from "@/components/ui/Menu";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { Segmented } from "@/components/ui/Segmented";
import { Select } from "@/components/ui/Select";
import { Slider } from "@/components/ui/Slider";
import { Switch } from "@/components/ui/Switch";
import { Tag } from "@/components/ui/Tag";
import { VirtualList } from "@/components/VirtualList";
import type { SolvedAccent } from "@/lib/color";
import type { EffectsLevel } from "@/lib/effects";
import {
  applyAccent,
  applyRadius,
  DEFAULT_ACCENT,
  RADIUS_PRESETS,
  resetThemeTokens,
} from "@/lib/themeEngine";
import { useAppStore } from "@/stores/app";

type Scheme = "light" | "dark";
type RadiusPreset = keyof typeof RADIUS_PRESETS;

const ACCENTS = [
  { name: "天空蓝（默认）", hue: 245, chroma: 0.15 },
  { name: "樱粉", hue: 350, chroma: 0.15 },
  { name: "薰衣草", hue: 300, chroma: 0.15 },
  { name: "草地绿", hue: 150, chroma: 0.15 },
  { name: "落日橙", hue: 55, chroma: 0.15 },
  { name: "柠檬黄", hue: 100, chroma: 0.15 },
];

const SWATCHES = [
  { name: "底色 bg", className: "bg-bg" },
  { name: "表面 surface", className: "bg-surface" },
  { name: "表面 2", className: "bg-surface-2" },
  { name: "浮层 overlay", className: "bg-overlay" },
  { name: "悬停", className: "bg-surface-hover" },
  { name: "线 line", className: "bg-line" },
  { name: "文字 fg", className: "bg-fg" },
  { name: "次要文字", className: "bg-fg-muted" },
  { name: "主色 accent", className: "bg-accent" },
  { name: "主色悬停", className: "bg-accent-hover" },
  { name: "主色浅底", className: "bg-accent-soft" },
  { name: "危险 danger", className: "bg-danger" },
  { name: "提醒 warning", className: "bg-warning" },
  { name: "遮罩 scrim", className: "bg-scrim" },
];

const RADII = [
  { name: "row 列表行、图标", className: "rounded-row" },
  { name: "control 按钮、输入框、下拉框（胶囊）", className: "rounded-control" },
  { name: "panel 面板、菜单", className: "rounded-panel" },
  { name: "dialog 弹窗", className: "rounded-dialog" },
  { name: "cta 主按钮", className: "rounded-cta" },
  { name: "icon 图标按钮、头像", className: "rounded-icon" },
  { name: "tag 标签", className: "rounded-tag" },
];

const MOD_NAMES = [
  "Sodium",
  "Iris Shaders",
  "Jade",
  "Waystones",
  "Architectury API",
  "Balm",
  "AppleSkin",
];
const MODS = Array.from({ length: 60 }, (_, i) => ({
  id: `mod-${i}`,
  name: MOD_NAMES[i % MOD_NAMES.length] ?? "Mod",
  version: `${1 + (i % 5)}.${i % 10}.${(i * 7) % 13}`,
  enabled: i % 6 !== 2,
  update: i % 9 === 3,
}));

function SliderDemo() {
  const [mb, setMb] = useState(6144);
  return (
    <Panel className="flex flex-col gap-5">
      <div className="flex items-center gap-4">
        <Slider
          value={mb}
          min={1024}
          max={16384}
          step={512}
          onChange={setMb}
          label="给游戏的内存"
          valueText={`${mb / 1024} GB`}
        />
        <span className="w-16 shrink-0 text-right font-medium tabular-nums">{mb / 1024} GB</span>
      </div>
      <MemoryBar totalMb={16384} availableMb={7168} gameMb={mb} />
      <Slider
        value={4096}
        min={1024}
        max={16384}
        step={512}
        onChange={() => {}}
        label="锁住"
        disabled
      />
    </Panel>
  );
}

function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="font-semibold text-lg">{title}</h2>
        {note && <p className="text-fg-muted text-sm">{note}</p>}
      </div>
      {children}
    </section>
  );
}

function Panel({ className = "", children }: { className?: string; children: ReactNode }) {
  return (
    <div className={`rounded-panel border border-line bg-surface p-5 ${className}`}>{children}</div>
  );
}

const pct = (v: number) => `${Math.round(v * 1000) / 10}%`;

export function ComponentsPage() {
  const html = document.documentElement;
  const initial = useRef({ theme: html.dataset.theme, fx: html.dataset.fx });
  const [scheme, setScheme] = useState<Scheme>(html.dataset.theme === "dark" ? "dark" : "light");
  const [fx, setFx] = useState<EffectsLevel>(html.dataset.fx === "lite" ? "lite" : "rich");
  const [hue, setHue] = useState<number>(DEFAULT_ACCENT.hue);
  const [chroma, setChroma] = useState<number>(DEFAULT_ACCENT.chroma);
  const [radius, setRadius] = useState<RadiusPreset>("tiered");
  const [solved, setSolved] = useState<SolvedAccent | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [progress, setProgress] = useState(0.35);
  const [replay, setReplay] = useState(0);
  const [selected, setSelected] = useState("mod-0");
  const [toggles, setToggles] = useState<Record<string, boolean>>({});
  const [segment, setSegment] = useState("mods");
  const [select, setSelect] = useState("auto");
  const [switchOn, setSwitchOn] = useState(true);

  useEffect(() => {
    html.dataset.theme = scheme;
  }, [html, scheme]);
  useEffect(() => {
    html.dataset.fx = fx;
  }, [html, fx]);
  useEffect(() => setSolved(applyAccent(hue, chroma)), [hue, chroma]);
  useEffect(() => applyRadius(RADIUS_PRESETS[radius]), [radius]);
  // 离开展示页：回到设置里的主题和底层外观
  useEffect(
    () => () => {
      resetThemeTokens();
      const { theme, fx: level } = initial.current;
      if (theme) html.dataset.theme = theme;
      if (level) html.dataset.fx = level;
    },
    [html],
  );

  const rows = useMemo(
    () => MODS.map((m) => ({ ...m, enabled: toggles[m.id] ?? m.enabled })),
    [toggles],
  );

  return (
    <MotionConfig reducedMotion={fx === "lite" ? "always" : "user"}>
      <div className="mx-auto flex max-w-5xl flex-col gap-10 p-8 pb-24">
        <header className="flex flex-col gap-1">
          <h1 className="font-semibold text-[28px]">组件展示页</h1>
          <p className="text-fg-muted text-sm">
            开发版专用。下面的开关只影响这一页，离开后回到设置里的样子。圆角、主色、画面效果都是主题引擎的数据，以后的主题包就是改这些。
          </p>
        </header>

        {/* 控制条 */}
        <Panel className="sticky top-4 z-20 flex flex-col gap-4 bg-overlay shadow-overlay">
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
            <Segmented<Scheme>
              label="深浅色"
              value={scheme}
              options={[
                { value: "light", label: "浅色" },
                { value: "dark", label: "深色" },
              ]}
              onChange={setScheme}
            />
            <Segmented<EffectsLevel>
              label="画面效果"
              value={fx}
              options={[
                { value: "rich", label: "精美" },
                { value: "lite", label: "流畅（低配）" },
              ]}
              onChange={setFx}
            />
            <Segmented<RadiusPreset>
              label="圆角"
              value={radius}
              options={[
                { value: "small", label: "全部小圆角" },
                { value: "tiered", label: "分级（默认）" },
                { value: "round", label: "全部大圆角" },
              ]}
              onChange={setRadius}
            />
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
            <div className="flex flex-wrap gap-2">
              {ACCENTS.map((a) => (
                <Button
                  key={a.name}
                  size="sm"
                  variant={a.hue === hue ? "primary" : "secondary"}
                  onClick={() => {
                    setHue(a.hue);
                    setChroma(a.chroma);
                  }}
                >
                  {a.name}
                </Button>
              ))}
            </div>
            <label className="flex items-center gap-2 text-sm">
              色相 {hue}
              <input
                type="range"
                min={0}
                max={359}
                step={1}
                value={hue}
                onChange={(e) => setHue(Number(e.target.value))}
                className="w-40 accent-accent"
              />
            </label>
            <label className="flex items-center gap-2 text-sm">
              彩度 {chroma.toFixed(2)}
              <input
                type="range"
                min={0.04}
                max={0.25}
                step={0.01}
                value={chroma}
                onChange={(e) => setChroma(Number(e.target.value))}
                className="w-32 accent-accent"
              />
            </label>
          </div>
          {solved && (
            <p className="font-mono text-fg-muted text-xs">
              浅色：亮度 {pct(solved.lightL)} 彩度 {solved.lightC.toFixed(3)} 对比度{" "}
              {solved.lightContrast.toFixed(2)} ｜ 深色：亮度 {pct(solved.darkL)} 彩度{" "}
              {solved.darkC.toFixed(3)} 对比度 {solved.darkContrast.toFixed(2)} ｜ 要求 ≥ 4.5
            </p>
          )}
        </Panel>

        <Section
          title="颜色令牌"
          note="组件里只能用这些名字，不能写颜色值。主色由色相、彩度两个数算出来，亮度保证对比度合格。"
        >
          <div className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-3">
            {SWATCHES.map((s) => (
              <div key={s.name} className="overflow-hidden rounded-panel border border-line">
                <div className={`h-14 ${s.className}`} />
                <p className="bg-surface px-3 py-2 text-xs">{s.name}</p>
              </div>
            ))}
          </div>
        </Section>

        <Section
          title="圆角分级"
          note="越密越小，越重要越圆。套在里面的圆角自动等于外面的减去间距。"
        >
          <div className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-3">
            {RADII.map((r) => (
              <div key={r.name} className="flex flex-col items-center gap-2">
                <div
                  className={`h-16 w-full border-2 border-accent bg-accent-soft ${r.className}`}
                />
                <p className="text-fg-muted text-xs">{r.name}</p>
              </div>
            ))}
          </div>
        </Section>

        <Section title="按钮">
          <Panel className="flex flex-col gap-5">
            <div className="flex flex-wrap items-center gap-3">
              <Button variant="primary">
                <Download className="size-4" aria-hidden />
                安装
              </Button>
              <Button variant="secondary">检查更新</Button>
              <Button variant="ghost">取消</Button>
              <Button variant="danger">删除</Button>
              <Button variant="primary" disabled>
                不能点
              </Button>
            </div>
            <div className="flex flex-wrap items-center gap-3">
              <Button variant="primary" size="sm">
                小按钮
              </Button>
              <Button variant="secondary" size="sm">
                <RefreshCw className="size-3.5" aria-hidden />
                刷新
              </Button>
              <Button variant="ghost" size="sm">
                查看日志
              </Button>
            </div>
            <div className="flex flex-wrap items-center gap-3">
              <IconButton label="设置">
                <Settings className="size-4" aria-hidden />
              </IconButton>
              <IconButton label="删除" danger>
                <Trash2 className="size-4" aria-hidden />
              </IconButton>
              <IconButton label="关闭" variant="secondary">
                <X className="size-4" aria-hidden />
              </IconButton>
              <IconButton label="小号" size="sm">
                <MoreHorizontal className="size-4" aria-hidden />
              </IconButton>
              <Menu
                label="更多"
                items={[
                  {
                    key: "a",
                    label: "打开文件夹",
                    icon: Package,
                    onSelect: () => toast("打开文件夹"),
                  },
                  {
                    key: "b",
                    label: "检查更新",
                    icon: RefreshCw,
                    onSelect: () => toast("检查更新"),
                  },
                  { key: "c", label: "删除", icon: Trash2, onSelect: () => toast.error("删除") },
                ]}
              />
            </div>
          </Panel>
        </Section>

        <Section
          title="主按钮（一页只有一个）"
          note="胶囊形，比别的按钮都大都亮；放在截图上靠一圈半透明白边和投影分开，不用毛玻璃。"
        >
          <div
            className="relative flex min-h-56 items-end justify-between gap-6 overflow-hidden rounded-panel p-8"
            style={{
              background: "linear-gradient(165deg, var(--accent) 0%, var(--surface-2) 100%)",
            }}
          >
            <div className="absolute inset-0 bg-[linear-gradient(0deg,var(--hero-scrim),transparent_65%)]" />
            <div className="relative text-on-hero">
              <p className="text-on-hero-muted text-sm">继续你的世界 · 验收测试</p>
              <p className="font-semibold text-[44px] leading-tight">新的世界</p>
              <div className="mt-2 flex gap-2">
                <Tag tone="hero">第 128 天</Tag>
                <Tag tone="hero">生存</Tag>
                <Tag tone="hero">玩了 3.2 小时</Tag>
              </div>
            </div>
            <div className="relative flex items-center gap-3">
              <CtaButton label="继续" sub="直接进入「新的世界」" onClick={() => toast("继续")} />
              <IconButton label="更多" variant="hero" size="lg">
                <MoreHorizontal className="size-5" aria-hidden />
              </IconButton>
            </div>
          </div>
          <Panel className="flex flex-wrap items-center gap-4">
            <CtaButton size="md" label="开始游戏" />
            <CtaButton
              size="md"
              variant="neutral"
              label="结束"
              icon={<X className="size-4" aria-hidden />}
            />
            <CtaButton size="md" label="开始游戏" disabled />
          </Panel>
        </Section>

        <Section title="输入">
          <Panel className="grid grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-6">
            <Field label="实例名" hint="名称便于以后识别">
              <TextInput defaultValue="1.21.1-NeoForge 21.1.265" />
            </Field>
            <Field label="最大内存">
              <Select
                label="最大内存"
                value={select}
                onChange={setSelect}
                options={[
                  { value: "auto", label: "自动（推荐）" },
                  ...[2, 3, 4, 6, 8, 12].map((gb) => ({ value: String(gb), label: `${gb} GB` })),
                ]}
              />
            </Field>
            <div className="flex flex-col gap-3">
              <Segmented
                label="内容类型"
                value={segment}
                onChange={setSegment}
                options={[
                  { value: "mods", label: "模组 8" },
                  { value: "shaders", label: "光影" },
                  { value: "packs", label: "材质包" },
                ]}
              />
              <div className="flex items-center gap-3 text-sm">
                <Switch label="开关" checked={switchOn} onChange={setSwitchOn} />
                {switchOn ? "开" : "关"}
              </div>
            </div>
          </Panel>
        </Section>

        <Section
          title="滑块和内存条"
          note="内存条三层叠在一起各自缩放（只动 transform）；游戏要的比可用多时那一段变成警告色。"
        >
          <SliderDemo />
        </Section>

        <Section title="标签和进度">
          <Panel className="flex flex-col gap-5">
            <div className="flex flex-wrap gap-2">
              <Tag>已停用</Tag>
              <Tag tone="accent">有更新</Tag>
              <Tag tone="accent">仅客户端</Tag>
              <Tag tone="warning">缺前置</Tag>
              <Tag tone="danger">崩溃</Tag>
            </div>
            <div className="flex items-center gap-4">
              <div className="flex-1">
                <ProgressBar value={progress} label="下载进度" />
              </div>
              <Button size="sm" onClick={() => setProgress((p) => (p >= 1 ? 0.05 : p + 0.2))}>
                推进
              </Button>
            </div>
            <div className="flex items-center gap-3 rounded-panel border border-line bg-overlay px-4 py-3 shadow-overlay">
              <Play className="size-5 text-accent" aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium text-sm">正在启动 验收测试</p>
                <p className="truncate text-fg-muted text-xs">检查游戏文件 1204/3187</p>
              </div>
              <Button variant="ghost" size="sm">
                查看日志
              </Button>
            </div>
          </Panel>
        </Section>

        <Section
          title="列表（超过 30 项一律虚拟滚动）"
          note="密的地方圆角最小；选中是主色浅底加一条小竖线；行里不放毛玻璃和大阴影。"
        >
          <div className="rounded-panel border border-line bg-surface p-1.5">
            <VirtualList
              className="h-80"
              items={rows}
              estimate={52}
              getKey={(m) => m.id}
              render={(m) => (
                <div
                  className={`relative flex items-center gap-3 rounded-row px-3 py-2 ${
                    selected === m.id ? "bg-accent-soft" : "hover:bg-surface-hover"
                  } ${m.enabled ? "" : "text-fg-muted"}`}
                  onMouseEnter={() => setSelected(m.id)}
                >
                  {selected === m.id && (
                    <span className="absolute top-2 bottom-2 left-0 w-[3px] rounded-tag bg-accent" />
                  )}
                  <span className="grid size-9 shrink-0 place-items-center rounded-row bg-accent-soft font-semibold text-accent text-xs">
                    {m.name.slice(0, 2)}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-sm">{m.name}</span>
                  {m.update && <Tag tone="accent">有更新</Tag>}
                  <span className="w-20 text-right font-mono text-fg-muted text-xs">
                    {m.version}
                  </span>
                  <Switch
                    label={m.name}
                    checked={m.enabled}
                    onChange={(on) => setToggles((t) => ({ ...t, [m.id]: on }))}
                  />
                </div>
              )}
            />
          </div>
        </Section>

        <Section title="浮层">
          <Panel className="flex flex-wrap items-center gap-3">
            <Button onClick={() => setDialogOpen(true)}>打开弹窗</Button>
            <Button onClick={() => useAppStore.getState().openSettings()}>打开设置弹窗</Button>
            <Button onClick={() => toast.success("已经回去了（4 项改动）")}>成功提示</Button>
            <Button onClick={() => toast.error("下载失败，请检查网络后重试")}>失败提示</Button>
          </Panel>
          <Dialog
            open={dialogOpen}
            onClose={() => setDialogOpen(false)}
            title="改动后崩溃了，回到上次能玩的状态？"
            footer={
              <>
                <Button variant="ghost" onClick={() => setDialogOpen(false)}>
                  先不用
                </Button>
                <Button variant="primary" onClick={() => setDialogOpen(false)}>
                  回到能玩的状态
                </Button>
              </>
            }
          >
            <p className="text-fg-muted text-sm">
              「验收测试」在 3 小时前还能正常玩。从那以后改了下面这些，然后游戏崩溃了。
            </p>
          </Dialog>
        </Section>

        <Section title="空状态">
          <EmptyState
            icon={Package}
            title="还没有模组"
            hint="去资源中心找一个装上，或者把模组文件拖进窗口。"
            action={<Button variant="primary">去资源中心</Button>}
          />
        </Section>

        <Section title="进场动效" note="一个接一个出现，间隔 30 毫秒；流畅模式下只淡入，不位移。">
          <Button className="self-start" onClick={() => setReplay((r) => r + 1)}>
            重播
          </Button>
          <Stagger key={replay} className="grid grid-cols-4 gap-3">
            {Array.from({ length: 8 }, (_, i) => (
              <StaggerItem key={`card-${i + 1}`}>
                <div className="h-20 rounded-panel border border-line bg-surface" />
              </StaggerItem>
            ))}
          </Stagger>
        </Section>
      </div>
    </MotionConfig>
  );
}
