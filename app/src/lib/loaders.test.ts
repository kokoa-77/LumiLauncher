import { describe, expect, it } from "vitest";
import { fallbackLoaderChoice, isLoaderCompatible } from "./loaders";

describe("loaders compatibility matrix", () => {
  it("matches backend compatibility for modern, snapshot, legacy, and beta versions", () => {
    expect(isLoaderCompatible("neoforge", "1.21.1")).toBe(true);
    expect(isLoaderCompatible("neoforge", "1.20.1")).toBe(true);
    expect(isLoaderCompatible("neoforge", "26.1")).toBe(true);
    expect(isLoaderCompatible("neoforge", "1.19.2")).toBe(false);
    expect(isLoaderCompatible("neoforge", "26.4-pre-1")).toBe(false);

    expect(isLoaderCompatible("forge", "1.21.1")).toBe(true);
    expect(isLoaderCompatible("forge", "1.12.2")).toBe(true);
    expect(isLoaderCompatible("forge", "1.7.10")).toBe(true);
    expect(isLoaderCompatible("forge", "1.5.2")).toBe(true);
    expect(isLoaderCompatible("forge", "1.5.1")).toBe(false);
    expect(isLoaderCompatible("forge", "b1.7.3")).toBe(false);

    expect(isLoaderCompatible("fabric", "1.21.1")).toBe(true);
    expect(isLoaderCompatible("fabric", "26.4-pre-1")).toBe(true);
    expect(isLoaderCompatible("fabric", "24w14potato")).toBe(true);
    expect(isLoaderCompatible("fabric", "1.12.2")).toBe(false);

    expect(isLoaderCompatible("legacyfabric", "1.8.9")).toBe(true);
    expect(isLoaderCompatible("legacyfabric", "1.3.2")).toBe(true);
    expect(isLoaderCompatible("legacyfabric", "1.14")).toBe(false);

    expect(isLoaderCompatible("babric", "b1.7.3")).toBe(true);
    expect(isLoaderCompatible("babric", "1.7.10")).toBe(false);
  });

  it("falls back incompatible loader choices to vanilla cleanly", () => {
    expect(fallbackLoaderChoice("neoforge", "b1.7.3")).toBe("vanilla");
    expect(fallbackLoaderChoice("babric", "b1.7.3")).toBe("babric");
    expect(fallbackLoaderChoice("forge", "1.7.10")).toBe("forge");
  });
});
