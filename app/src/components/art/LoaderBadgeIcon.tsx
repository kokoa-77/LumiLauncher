/**
 * 24×24 高清原创加载器识别徽章（纯矢量 SVG，16px–24px 下都轮廓分明）：
 * 放在实例卡片左侧徽章格与详情页版本行首位，一眼认出 NeoForge / Fabric / Forge / Quilt / 原版。
 */
export function LoaderBadgeIcon({
  loader,
  title,
  className = "size-5 shrink-0",
}: {
  /** 实例的加载器显示字符串（如 `"Fabric 0.15.6"`、`"NeoForge 21.1.244"`），原版为 null */
  loader: string | null | undefined;
  title?: string;
  className?: string;
}) {
  const kind = loader?.trim().split(/\s+/)[0]?.toLowerCase() ?? "vanilla";

  if (kind === "neoforge") {
    // 原创橙狐狸徽章（NeoForge 意象）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        {/* 双耳 */}
        <path
          d="M4.5 9.5L5.5 3.5L10 7.2Z"
          fill="#7c5340"
          stroke="#3b261d"
          strokeWidth="1.1"
          strokeLinejoin="round"
        />
        <path
          d="M19.5 9.5L18.5 3.5L14 7.2Z"
          fill="#7c5340"
          stroke="#3b261d"
          strokeWidth="1.1"
          strokeLinejoin="round"
        />
        <path d="M6.2 8.2L6.6 5.3L8.9 7.3Z" fill="#c98e75" />
        <path d="M17.8 8.2L17.4 5.3L15.1 7.3Z" fill="#c98e75" />
        {/* 头部主体 */}
        <path
          d="M4 10.5C4 7.8 7.2 6.2 12 6.2C16.8 6.2 20 7.8 20 10.5V14.2C20 17.5 16.6 20 12 20C7.4 20 4 17.5 4 14.2V10.5Z"
          fill="#e26e26"
          stroke="#4a2510"
          strokeWidth="1.1"
        />
        {/* 额头高光与颊侧绒毛 */}
        <path d="M10.5 6.8H13.5L12.8 11.2H11.2Z" fill="#f79a44" />
        <path
          d="M5.5 14.8C6.6 17.8 9.2 19.3 12 19.3C14.8 19.3 17.4 17.8 18.5 14.8L15.5 14.2L12 15.8L8.5 14.2Z"
          fill="#fff7ed"
        />
        {/* 双眼 */}
        <rect x="6.8" y="10.8" width="3.2" height="3.2" rx="0.8" fill="#ffffff" />
        <rect x="8.1" y="11.5" width="1.6" height="2.2" rx="0.4" fill="#182032" />
        <rect x="14" y="10.8" width="3.2" height="3.2" rx="0.8" fill="#ffffff" />
        <rect x="14.3" y="11.5" width="1.6" height="2.2" rx="0.4" fill="#182032" />
        {/* 黑鼻头 */}
        <rect x="10.4" y="15.2" width="3.2" height="2.2" rx="0.9" fill="#182032" />
      </svg>
    );
  }

  if (kind === "fabric") {
    // 原创高清羊皮卷轴（清晰双卷边 + 卷芯阴影 + 暖米纸面）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        {/* 卷轴展开纸身（斜 45°） */}
        <path
          d="M12.8 4.4L19.6 11.2L11.2 19.6L4.4 12.8Z"
          fill="#ebe0c3"
          stroke="#4b4231"
          strokeWidth="1.4"
          strokeLinejoin="round"
        />
        {/* 纸面阴影层次与织纹 */}
        <path d="M15.4 7L18.4 10L10.6 17.8L7.6 14.8Z" fill="#d9cba4" />
        <path
          d="M10.8 9.6L14.4 13.2M8.8 11.6L11.8 14.6"
          stroke="#8c7d5c"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
        {/* 右上卷边（圆筒卷芯） */}
        <path
          d="M11.6 3.6C12.7 2.5 14.4 2.5 15.4 3.6L20.4 8.6C21.5 9.6 21.5 11.3 20.4 12.4C19.3 13.5 17.6 13.5 16.6 12.4L11.6 7.4C10.5 6.4 10.5 4.7 11.6 3.6Z"
          fill="#f6eed8"
          stroke="#4b4231"
          strokeWidth="1.4"
        />
        <circle cx="18.5" cy="10.5" r="1.4" fill="#635742" />
        {/* 左下卷边 */}
        <path
          d="M3.6 11.6C4.7 10.5 6.4 10.5 7.4 11.6L12.4 16.6C13.5 17.6 13.5 19.3 12.4 20.4C11.3 21.5 9.6 21.5 8.6 20.4L3.6 15.4C2.5 14.4 2.5 12.7 3.6 11.6Z"
          fill="#dfd2ae"
          stroke="#4b4231"
          strokeWidth="1.4"
        />
        <circle cx="5.5" cy="13.5" r="1.2" fill="#635742" />
      </svg>
    );
  }

  if (kind === "forge") {
    // 原创高清钢制铁砧（Forge 意象）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        <path
          d="M2.5 7.5H21.5L19 12H7L2.5 9.5V7.5Z"
          fill="#cbd5e1"
          stroke="#334155"
          strokeWidth="1.3"
          strokeLinejoin="round"
        />
        <path d="M4 8.5H20" stroke="#f8fafc" strokeWidth="1.2" strokeLinecap="round" />
        <path
          d="M9.2 12H14.8L16 16H8L9.2 12Z"
          fill="#64748b"
          stroke="#334155"
          strokeWidth="1.3"
          strokeLinejoin="round"
        />
        <path
          d="M5.5 16H18.5L20 19.5H4L5.5 16Z"
          fill="#94a3b8"
          stroke="#334155"
          strokeWidth="1.3"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  if (kind === "legacyfabric" || kind === "legacy") {
    // 原创复古丝织羊皮卷轴（Legacy Fabric 1.3.2–1.13.2 意象）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        <path
          d="M12.8 4.4L19.6 11.2L11.2 19.6L4.4 12.8Z"
          fill="#e2cca3"
          stroke="#5c4228"
          strokeWidth="1.4"
          strokeLinejoin="round"
        />
        <path d="M15.4 7L18.4 10L10.6 17.8L7.6 14.8Z" fill="#c7ab7c" />
        <path
          d="M10.4 9.6L14.2 13.4M8.5 11.5L12.3 15.3"
          stroke="#7a5634"
          strokeWidth="1.3"
          strokeLinecap="round"
        />
        <path
          d="M11.6 3.6C12.7 2.5 14.4 2.5 15.4 3.6L20.4 8.6C21.5 9.6 21.5 11.3 20.4 12.4C19.3 13.5 17.6 13.5 16.6 12.4L11.6 7.4C10.5 6.4 10.5 4.7 11.6 3.6Z"
          fill="#f2e1be"
          stroke="#5c4228"
          strokeWidth="1.4"
        />
        <circle cx="18.5" cy="10.5" r="1.4" fill="#9a5b28" />
        <path
          d="M3.6 11.6C4.7 10.5 6.4 10.5 7.4 11.6L12.4 16.6C13.5 17.6 13.5 19.3 12.4 20.4C11.3 21.5 9.6 21.5 8.6 20.4L3.6 15.4C2.5 14.4 2.5 12.7 3.6 11.6Z"
          fill="#cbb184"
          stroke="#5c4228"
          strokeWidth="1.4"
        />
        <circle cx="5.5" cy="13.5" r="1.2" fill="#9a5b28" />
      </svg>
    );
  }

  if (kind === "babric") {
    // 原创远古 Beta b1.7.3 怀旧织方（Beta 草木棕绿 + 暖织线网格）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        <rect
          x="3.5"
          y="3.5"
          width="17"
          height="17"
          rx="3"
          fill="#d9c69e"
          stroke="#463726"
          strokeWidth="1.3"
        />
        <path d="M4.2 8.5H19.8" stroke="#463726" strokeWidth="1.2" />
        <rect x="4.2" y="4.2" width="15.6" height="4.2" rx="1.8" fill="#4f9d53" />
        <path
          d="M7.5 11.5H16.5M7.5 14.5H16.5M7.5 17.5H13.5"
          stroke="#7d5836"
          strokeWidth="1.4"
          strokeLinecap="round"
        />
      </svg>
    );
  }

  if (kind === "quilt") {
    // 原创紫韵拼布（Quilt 意象）
    return (
      <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
        {title && <title>{title}</title>}
        <rect
          x="3.5"
          y="3.5"
          width="8"
          height="8"
          rx="2"
          fill="#a855f7"
          stroke="#3b1d5e"
          strokeWidth="1.2"
        />
        <rect
          x="12.5"
          y="3.5"
          width="8"
          height="8"
          rx="2"
          fill="#ec4899"
          stroke="#3b1d5e"
          strokeWidth="1.2"
        />
        <rect
          x="3.5"
          y="12.5"
          width="8"
          height="8"
          rx="2"
          fill="#6366f1"
          stroke="#3b1d5e"
          strokeWidth="1.2"
        />
        <rect
          x="12.5"
          y="12.5"
          width="8"
          height="8"
          rx="2"
          fill="#c084fc"
          stroke="#3b1d5e"
          strokeWidth="1.2"
        />
        <circle cx="7.5" cy="7.5" r="1.4" fill="#f3e8ff" />
        <circle cx="16.5" cy="16.5" r="1.4" fill="#fdf2f8" />
      </svg>
    );
  }

  // 原版立体草方块
  return (
    <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden={!title}>
      {title && <title>{title}</title>}
      <polygon
        points="12,3 20.5,7.8 12,12.6 3.5,7.8"
        fill="#68cf55"
        stroke="#2c5924"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <polygon
        points="3.5,7.8 12,12.6 12,21 3.5,16.2"
        fill="#9a643d"
        stroke="#3d2616"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <polygon
        points="20.5,7.8 12,12.6 12,21 20.5,16.2"
        fill="#774b2c"
        stroke="#3d2616"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <path d="M3.5 7.8L12 12.6L20.5 7.8V10.8L12 15.6L3.5 10.8V7.8Z" fill="#52b342" />
    </svg>
  );
}
