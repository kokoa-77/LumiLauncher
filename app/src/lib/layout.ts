// 页面容器（D17f）：所有页面同一个最大宽度、同样的左右和顶部留白，切页时内容不会左右跳。
// 资源中心的虚拟列表铺满整个宽度（滚动条贴着窗口右边），每一行里面再套 PAGE_X。

/** 页面内容的最大宽度（1152 像素）和左右留白（32） */
export const PAGE_X = "mx-auto w-full max-w-6xl px-8";
/** 普通页面：PAGE_X + 上下留白 */
export const PAGE = `${PAGE_X} py-8`;
/** 内容的最大宽度（像素，去掉左右留白），资源中心按它算一排放几张卡片 */
export const PAGE_CONTENT_MAX = 1152 - 64;

/** 沉浸首页的整页布局和底部操作排布。 */
export const HOME_STAGE =
  "relative isolate flex h-full min-h-[480px] flex-col justify-end overflow-hidden";
export const HOME_STAGE_CONTENT = "flex items-end gap-8 px-12 pt-10 pb-12";
