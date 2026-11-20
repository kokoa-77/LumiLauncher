// 补件（作者禁止第三方下载的文件由用户在浏览器里下载）的状态。后端事件在 initClaimEvents 里订阅。
import { toast } from "sonner";
import { create } from "zustand";
import { type ClaimSnapshot, commands, events } from "@/lib/bindings";
import i18n from "@/lib/i18n";

interface ClaimStore {
  /** 正在进行的补件（结束的会被删掉） */
  claims: Record<string, ClaimSnapshot>;
  /** 补件面板显示的是哪一个；null 表示收起 */
  shown: string | null;
  /** 有文件正被拖到窗口上方 */
  dragging: boolean;
  show: (id: string | null) => void;
}

export const useClaimStore = create<ClaimStore>((set) => ({
  claims: {},
  shown: null,
  dragging: false,
  show(id) {
    set({ shown: id });
  },
}));

/** 必需的都认领了、可选的认领或跳过了 */
export function isComplete(claim: ClaimSnapshot): boolean {
  return claim.entries.every((e) => e.state.kind === "claimed" || e.state.kind === "skipped");
}

/** 已经处理好的（认领或跳过）个数 */
export function settledCount(claim: ClaimSnapshot): number {
  return claim.entries.filter((e) => e.state.kind === "claimed" || e.state.kind === "skipped")
    .length;
}

/** 补齐后提示过的，不再重复提示 */
const announced = new Set<string>();

function upsert(claim: ClaimSnapshot) {
  const store = useClaimStore;
  const { claims, shown } = store.getState();
  if (claim.closed) {
    const { [claim.id]: _, ...rest } = claims;
    store.setState({ claims: rest, shown: shown === claim.id ? null : shown });
    return;
  }
  const isNew = !(claim.id in claims);
  const complete = isComplete(claim);
  let nextShown = shown;
  // 新开始的补件自动弹出面板；补齐了就收起，安装在后台继续
  if (isNew && !complete) nextShown = claim.id;
  if (complete && !announced.has(claim.id)) {
    announced.add(claim.id);
    if (!isNew) toast.success(i18n.t("claim.complete", { name: claim.title }));
    if (nextShown === claim.id) nextShown = null;
  }
  store.setState({ claims: { ...claims, [claim.id]: claim }, shown: nextShown });
}

/** 订阅补件事件并读取当前状态。返回取消订阅的函数。 */
export async function initClaimEvents(): Promise<() => void> {
  const store = useClaimStore;
  const unlisten = await Promise.all([
    events.claimUpdated.listen(({ payload }) => upsert(payload)),
    events.fileDragHover.listen(({ payload }) => {
      const { claims, shown } = store.getState();
      // 拖文件进来时面板收着，就把最新的一个补件打开，让用户看到往哪放
      const open = Object.values(claims).filter((c) => !isComplete(c));
      const newest = open[open.length - 1];
      store.setState({
        dragging: payload,
        shown: payload && !shown && newest ? newest.id : shown,
      });
    }),
  ]);
  for (const claim of await commands.claimList()) upsert(claim);
  return () => {
    for (const stop of unlisten) stop();
  };
}
