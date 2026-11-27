import { renderToStaticMarkup } from "react-dom/server";
import { beforeAll, describe, expect, it } from "vitest";
import { setLanguagePreference } from "@/lib/i18n";
import { AccountForm, AuthProgressPanel } from "./AccountForm";

beforeAll(async () => {
  await setLanguagePreference("zh-CN");
});

describe("Minecraft 登录弹窗", () => {
  it("未登录时显示说明、购买、登录入口及 OAuth 2.1 信任提示", () => {
    const markup = renderToStaticMarkup(<AccountForm onDone={() => undefined} />);
    expect(markup).toContain(
      "您尚未登录任何账户，请在下方登录。如果您尚未拥有 Minecraft，可以前往官方网站购买。",
    );
    expect(markup).toContain("购买 Minecraft");
    expect(markup).toContain("登录正版 Minecraft");
    expect(markup).toContain("OAuth 2.1 PKCE S256");
  });

  it("受阻原因为 auth.xbox_profile 时展示对应恢复引导链接", () => {
    const markup = renderToStaticMarkup(
      <AccountForm reason="auth.xbox_profile" onDone={() => undefined} />,
    );
    expect(markup).toContain("请先在 Xbox 完成账号设置");
    expect(markup).toContain("前往 Xbox 完成账号设置");
  });

  it("登录中四阶段进度面板展示进度与重开/复制操作", () => {
    const markup = renderToStaticMarkup(
      <AuthProgressPanel
        stage="waitingBrowser"
        cancelling={false}
        remainingMs={120_000}
        copiedUrl={false}
        onReopen={() => undefined}
        onCopyUrl={() => undefined}
      />,
    );
    expect(markup).toContain("浏览器授权");
    expect(markup).toContain("Xbox 签发");
    expect(markup).toContain("XSTS 票据");
    expect(markup).toContain("正版核验");
    expect(markup).toContain("02:00");
    expect(markup).toContain("重新打开浏览器");
    expect(markup).toContain("复制登录链接");
  });
});
