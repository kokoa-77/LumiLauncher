import { describe, expect, it } from "vitest";
import i18n, { getLanguagePreference, setLanguagePreference } from "./i18n";
import { jobProgressLabel, jobTitle } from "./jobLabels";

describe("任务显示文案", () => {
  it("切换中英文时保留历史记录中的原文与项目名", async () => {
    const previous = getLanguagePreference();
    const job = { key: "content:id:project", title: "安装 测试模组 等 2 项" };
    try {
      await setLanguagePreference("en");
      expect(jobTitle(job, i18n.t)).toBe("Install 测试模组 等 2 项");
      expect(job.title).toBe("安装 测试模组 等 2 项");
      await setLanguagePreference("zh-CN");
      expect(jobTitle(job, i18n.t)).toBe(job.title);
      expect(jobProgressLabel("补全游戏文件 3/15", i18n.t)).toBe("补全游戏文件 3/15");
    } finally {
      await setLanguagePreference(previous);
    }
  });

  it("翻译已知的进度与步骤并保留加载器任务名", async () => {
    const previous = getLanguagePreference();
    try {
      await setLanguagePreference("en");
      expect(jobProgressLabel("补全游戏文件 3/15", i18n.t)).toBe(
        "Downloading missing game files 3/15",
      );
      expect(jobProgressLabel("检查版本文件", i18n.t)).toBe("Checking version files");
      expect(jobProgressLabel("下载 Java 21 1/5", i18n.t)).toBe("Downloading Java 21 1/5");
      expect(jobProgressLabel("安装加载器 1/2：自定义程序", i18n.t)).toBe(
        "Installing mod loader 1/2: 自定义程序",
      );
      expect(jobProgressLabel("等你补齐 1 个文件（作者不允许第三方启动器下载）", i18n.t)).toBe(
        "Waiting for 1 manual download (author restrictions)",
      );
      expect(jobProgressLabel("某模组 3/4", i18n.t)).toBe("某模组 3/4");
      expect(jobProgressLabel(null, i18n.t)).toBeNull();
    } finally {
      await setLanguagePreference(previous);
    }
  });

  it("根据任务类型翻译标题并保留世界名和实例名", async () => {
    const previous = getLanguagePreference();
    try {
      await setLanguagePreference("en");
      expect(jobTitle({ key: "backup:id:world", title: "恢复世界「我的世界」" }, i18n.t)).toBe(
        "Restore world: 我的世界",
      );
      expect(jobTitle({ key: "drop:id:files", title: "装进 名字：副本：1 个文件" }, i18n.t)).toBe(
        "Add 1 file to 名字：副本",
      );
      expect(jobTitle({ key: "unknown:source", title: "安装 示例模组" }, i18n.t)).toBe(
        "安装 示例模组",
      );
    } finally {
      await setLanguagePreference(previous);
    }
  });
});
