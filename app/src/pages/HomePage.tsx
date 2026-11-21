import { Plus } from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { HomeHero, type HomeHeroTarget } from "@/components/HomeHero";
import { instanceSubtitle } from "@/components/InstanceCard";
import { PlayButton } from "@/components/PlayButton";
import { Stagger, StaggerItem } from "@/components/Stagger";
import { CtaButton } from "@/components/ui/CtaButton";
import { Truncated } from "@/components/ui/HoverCard";
import type { HomeTarget } from "@/components/WorldPicker";
import type { InstanceSummary } from "@/lib/bindings";
import { useCoverAccent, useWorldAccent } from "@/lib/cover";
import { HOME_STAGE, HOME_STAGE_CONTENT } from "@/lib/layout";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";

const NewInstanceDialog = lazy(() =>
  import("@/components/NewInstanceDialog").then((m) => ({ default: m.NewInstanceDialog })),
);

const HOME_TARGET_KEY = "mcl.homeTarget";

function readSavedTarget(): HomeTarget | null {
  try {
    const raw = localStorage.getItem(HOME_TARGET_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    const record = parsed as Record<string, unknown>;
    if (record.kind === "world" && typeof record.instanceId === "string") {
      return { kind: "world", instanceId: record.instanceId };
    }
    if (
      record.kind === "server" &&
      typeof record.instanceId === "string" &&
      typeof record.address === "string"
    ) {
      return { kind: "server", instanceId: record.instanceId, address: record.address };
    }
  } catch {
    // 忽略损坏的本地记忆
  }
  return null;
}

/** 只简化与版本信息完全匹配的自动名称，兼容旧实例的连字符和下划线。 */
function isAutomaticInstanceName({ name, gameVersion, loader, pack }: InstanceSummary): boolean {
  if (pack || !gameVersion) return false;
  if (name === gameVersion) return true;
  if (!loader) return false;

  const normalize = (value: string) => value.replace(/[\s_-]+/g, "-").toLowerCase();
  return normalize(name) === normalize(`${gameVersion}-${loader}`);
}

/** 首页优先接着玩最近的存档，也支持切换或直达多人服务器。没有世界与服务器时，仍沿用同一套首页布局。 */
export function HomePage() {
  const { t } = useTranslation();
  const instances = useGameStore((s) => s.instances);
  const loaded = useGameStore((s) => s.instancesLoaded);
  const recent = useGameStore((s) => s.recentWorlds);
  const recentServers = useGameStore((s) => s.recentServers);
  const setBackdrop = useAppStore((s) => s.setBackdrop);
  const [creating, setCreating] = useState(false);
  const [creatingLoaded, setCreatingLoaded] = useState(false);
  const [picked, setPicked] = useState<HomeTarget | null>(() => readSavedTarget());

  const handlePick = useCallback((next: HomeTarget) => {
    setPicked(next);
    try {
      localStorage.setItem(HOME_TARGET_KEY, JSON.stringify(next));
    } catch {
      // 忽略存储配额等异常
    }
  }, []);

  const instancesById = useMemo(
    () => new Map(instances.map((instance) => [instance.id, instance])),
    [instances],
  );
  const worlds = recent?.filter((world) => instancesById.has(world.instanceId)) ?? [];
  const servers = recentServers?.filter((server) => instancesById.has(server.instanceId)) ?? [];

  const target: HomeHeroTarget | null = useMemo(() => {
    if (picked?.kind === "world") {
      const found = worlds.find((w) => w.instanceId === picked.instanceId);
      if (found) return { kind: "world", entry: found };
    } else if (picked?.kind === "server") {
      const found = servers.find(
        (s) => s.instanceId === picked.instanceId && s.address === picked.address,
      );
      if (found) return { kind: "server", entry: found };
    }
    if (worlds[0]) return { kind: "world", entry: worlds[0] };
    if (servers[0]) return { kind: "server", entry: servers[0] };
    return null;
  }, [picked, worlds, servers]);

  const hero = target ? instancesById.get(target.entry.instanceId) : instances[0];
  // 选中多人服时，若该实例本身有单人世界封面则顺延作氛围背景，否则回落到 Lumi 默认插画。
  const backdropWorld =
    target?.kind === "world"
      ? target.entry.world
      : target?.kind === "server"
        ? worlds.find((w) => w.instanceId === target.entry.instanceId)?.world
        : undefined;
  const imageUrl = backdropWorld?.imageUrl ?? null;
  const imageKind = backdropWorld?.imageKind ?? null;
  const imageFit = backdropWorld?.imageFit ?? "cover";

  const color = useCoverAccent(imageUrl);
  useWorldAccent(recent !== null && recentServers !== null ? color : undefined);

  useLayoutEffect(() => {
    setBackdrop(imageUrl ? { url: imageUrl, kind: imageKind, fit: imageFit } : null);
  }, [imageUrl, imageKind, imageFit, setBackdrop]);
  useEffect(() => () => setBackdrop(null), [setBackdrop]);

  // 存档与服务器扫描期间保留窗口底色，扫完后再选用实际的封面或默认插画。
  if (!loaded || (instances.length > 0 && (recent === null || recentServers === null))) {
    return <section aria-busy className={HOME_STAGE} />;
  }

  if (target && hero) {
    return (
      <HomeHero
        instance={hero}
        target={target}
        worlds={worlds}
        servers={servers}
        instances={instances}
        onPick={handlePick}
      />
    );
  }

  const automaticName = hero && isAutomaticInstanceName(hero);
  const title = automaticName
    ? t("home.instance.title", { version: hero.gameVersion })
    : (hero?.name ?? t("home.emptyTitle"));
  const subtitle = hero && (automaticName ? hero.loader : instanceSubtitle(hero));

  return (
    <section className={HOME_STAGE}>
      <div className={HOME_STAGE_CONTENT}>
        <Stagger key={hero?.id ?? "empty"} className="min-w-0 flex-1 text-on-hero">
          {subtitle && (
            <StaggerItem>
              <p className="wrap-anywhere text-on-hero-muted text-sm">{subtitle}</p>
            </StaggerItem>
          )}
          <StaggerItem className="mt-1">
            <Truncated
              as="h1"
              className="line-clamp-2 wrap-anywhere font-semibold text-hero tracking-[-0.02em]"
              text={title}
            />
          </StaggerItem>
        </Stagger>
        <div className="shrink-0">
          {hero ? (
            <PlayButton instanceId={hero.id} large />
          ) : (
            <CtaButton
              icon={<Plus className="size-5" aria-hidden />}
              label={t("instances.create")}
              onClick={() => {
                setCreatingLoaded(true);
                setCreating(true);
              }}
            />
          )}
        </div>
      </div>
      {creatingLoaded && (
        <Suspense fallback={null}>
          <NewInstanceDialog open={creating} onClose={() => setCreating(false)} />
        </Suspense>
      )}
    </section>
  );
}
