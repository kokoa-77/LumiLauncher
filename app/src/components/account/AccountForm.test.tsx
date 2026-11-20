import { renderToStaticMarkup } from "react-dom/server";
import { beforeAll, describe, expect, it } from "vitest";
import { setLanguagePreference } from "@/lib/i18n";
import { AccountForm } from "./AccountForm";

beforeAll(async () => {
  await setLanguagePreference("zh-CN");
});

describe("Minecraft 登录弹窗", () => {
  it("未登录时显示说明及购买、登录入口", () => {
    const markup = renderToStaticMarkup(<AccountForm onDone={() => undefined} />);
    expect(markup).toContain(
      "您尚未登录任何账户，请在下方登录。如果您尚未拥有 Minecraft，可以前往官方网站购买。",
    );
    expect(markup).toContain("购买 Minecraft");
    expect(markup).toContain("登录正版 Minecraft");
  });
});
