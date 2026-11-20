import { describe, expect, it } from "vitest";
import { addArgs, hasArgs, picksCollector, removeArgs } from "./javaArgs";

describe("Java 参数预设", () => {
  it("加上的参数放在最后，已经有的不重复加", () => {
    expect(addArgs("", ["-XX:+DisableExplicitGC"])).toBe("-XX:+DisableExplicitGC");
    expect(addArgs(" -Xss4m ", ["-XX:+DisableExplicitGC"])).toBe("-Xss4m -XX:+DisableExplicitGC");
    expect(addArgs("-XX:+DisableExplicitGC", ["-XX:+DisableExplicitGC"])).toBe(
      "-XX:+DisableExplicitGC",
    );
  });

  it("去掉时别的参数原样留着（带引号的也不拆坏）", () => {
    const text = '-Dname="a b" -XX:+DisableExplicitGC -Xss4m';
    expect(removeArgs(text, ["-XX:+DisableExplicitGC"])).toBe('-Dname="a b" -Xss4m');
    expect(removeArgs("-XX:+DisableExplicitGC", ["-XX:+DisableExplicitGC"])).toBe("");
  });

  it("认得出写没写上", () => {
    expect(hasArgs("-Xss4m -Dfile.encoding=UTF-8", ["-Dfile.encoding=UTF-8"])).toBe(true);
    expect(hasArgs("-Dfile.encoding=UTF-8x", ["-Dfile.encoding=UTF-8"])).toBe(false);
  });

  it("和后端一样认自己写的垃圾回收器", () => {
    expect(picksCollector("-Xss4m -XX:+UseZGC")).toBe(true);
    expect(picksCollector("-XX:+UseShenandoahGC")).toBe(true);
    expect(picksCollector("-XX:+UseStringDeduplication")).toBe(false);
    expect(picksCollector("-XX:+DisableExplicitGC")).toBe(false);
    expect(picksCollector("-XX:+UseGC")).toBe(false);
  });
});
