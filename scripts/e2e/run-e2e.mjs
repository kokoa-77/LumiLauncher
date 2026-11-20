/**
 * Lumi 端到端自动化测试套件（D26 · tauri-driver + W3C WebDriver）
 *
 * 特点：
 * - 零新增第三方 npm 依赖（直接用 Node 原生 fetch 调 W3C WebDriver HTTP 协议）
 * - 100% 隔离便携沙盒（Lumi.portable + 独立临时 MCL_GAME_DIR），绝不触碰真实用户数据
 * - 覆盖：首次运行引导、首页世界与 D25a 登录拦截、世界分享卡片、实例库与详情页签、
 *         发现内容与面包屑导航、命令面板、关于页合规/弹窗叠层、语言与流畅模式切换、下载中心
 */

import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, "../..");
const KEEP_SANDBOX = process.argv.includes("--keep");

const DEFAULT_TAURI_DRIVER = "D:\\dev\\cargo\\bin\\tauri-driver.exe";
const DEFAULT_EDGE_DRIVER = "D:\\dev\\tools\\msedgedriver\\msedgedriver.exe";

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function findExecutable(name, fallbacks) {
  const pathDirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  for (const dir of pathDirs) {
    const candidate = path.join(dir, name);
    if (fs.existsSync(candidate)) return candidate;
  }
  for (const candidate of fallbacks) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function findAppBinary() {
  if (process.env.LUMI_E2E_BINARY && fs.existsSync(process.env.LUMI_E2E_BINARY)) {
    return process.env.LUMI_E2E_BINARY;
  }
  const candidates = [
    path.join(REPO_ROOT, "target", "debug", "launcher-app.exe"),
    path.join(REPO_ROOT, "target", "debug", "Lumi.exe"),
    path.join(REPO_ROOT, "target", "release", "launcher-app.exe"),
    path.join(REPO_ROOT, "target", "release", "Lumi.exe"),
  ];
  for (const p of candidates) {
    if (fs.existsSync(p)) return p;
  }
  return null;
}

/** 获取本机可用端口，避免端口冲突 */
function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      server.close(() => resolve(port));
    });
  });
}

/** 构造合法的 gzip 压缩 Minecraft NBT level.dat，供隔离沙盒测试首页与世界页签 */
function buildLevelDat({ levelName, lastPlayedMs, dayTimeTicks = 72000n, gameType = 0 }) {
  const chunks = [];
  const pushU8 = (n) => chunks.push(Buffer.from([n]));
  const pushI16BE = (n) => {
    const b = Buffer.alloc(2);
    b.writeInt16BE(n, 0);
    chunks.push(b);
  };
  const pushI32BE = (n) => {
    const b = Buffer.alloc(4);
    b.writeInt32BE(n, 0);
    chunks.push(b);
  };
  const pushI64BE = (n) => {
    const b = Buffer.alloc(8);
    b.writeBigInt64BE(BigInt(n), 0);
    chunks.push(b);
  };
  const pushName = (name) => {
    const buf = Buffer.from(name, "utf8");
    pushI16BE(buf.length);
    chunks.push(buf);
  };

  // Root compound ""
  pushU8(10);
  pushName("");

  // Data compound
  pushU8(10);
  pushName("Data");

  // LevelName (TAG_String = 8)
  pushU8(8);
  pushName("LevelName");
  pushName(levelName);

  // LastPlayed (TAG_Long = 4)
  pushU8(4);
  pushName("LastPlayed");
  pushI64BE(lastPlayedMs);

  // Time (TAG_Long = 4)
  pushU8(4);
  pushName("Time");
  pushI64BE(dayTimeTicks);

  // DayTime (TAG_Long = 4)
  pushU8(4);
  pushName("DayTime");
  pushI64BE(dayTimeTicks);

  // GameType (TAG_Int = 3)
  pushU8(3);
  pushName("GameType");
  pushI32BE(gameType);

  // End Data
  pushU8(0);
  // End Root
  pushU8(0);

  return zlib.gzipSync(Buffer.concat(chunks));
}

/** 创建 100% 隔离的便携版沙盒环境与样本实例 */
function createSandbox(appBinary) {
  const baseTemp = fs.existsSync("D:\\dev\\tmp") ? "D:\\dev\\tmp" : os.tmpdir();
  const sandboxDir = path.join(baseTemp, `lumi-e2e-${Date.now()}`);
  fs.mkdirSync(sandboxDir, { recursive: true });

  const exePath = path.join(sandboxDir, "Lumi.exe");
  fs.copyFileSync(appBinary, exePath);
  // 开启便携模式，把设置、日志、WebView2 数据全锁进 sandboxDir/LumiData
  fs.writeFileSync(path.join(sandboxDir, "Lumi.portable"), "");

  const gameDir = path.join(sandboxDir, "LumiData", "minecraft");
  const now = Date.now();

  // 样本实例 1：星夜生存（含一个合法单人世界「星夜营地」）
  const inst1 = path.join(gameDir, "instances", "starlight-survival");
  fs.mkdirSync(path.join(inst1, "Lumi"), { recursive: true });
  fs.mkdirSync(path.join(inst1, "saves", "starlight"), { recursive: true });
  fs.writeFileSync(
    path.join(inst1, "Lumi", "instance.json"),
    JSON.stringify(
      {
        format: 2,
        name: "星夜生存",
        gameVersion: "1.21.1",
        loader: { kind: "fabric", version: "0.16.10" },
        versionId: "fabric-loader-0.16.10-1.21.1",
        createdAt: now - 86_400_000,
        source: null,
        lastPlayed: now - 3_600_000,
        recommendedMemoryMb: null,
        overrides: { memory: null, javaId: null, launch: null, window: null, envVars: null },
        group: "生存",
        link: null,
      },
      null,
      2,
    ),
  );
  fs.writeFileSync(
    path.join(inst1, "saves", "starlight", "level.dat"),
    buildLevelDat({
      levelName: "星夜营地",
      lastPlayedMs: now - 3_600_000,
      dayTimeTicks: 96000n,
      gameType: 0,
    }),
  );

  // 样本实例 2：建筑沙盒（原版 1.21.1，用于测试实例库检索与分组）
  const inst2 = path.join(gameDir, "instances", "creative-sandbox");
  fs.mkdirSync(path.join(inst2, "Lumi"), { recursive: true });
  fs.writeFileSync(
    path.join(inst2, "Lumi", "instance.json"),
    JSON.stringify(
      {
        format: 2,
        name: "建筑沙盒",
        gameVersion: "1.21.1",
        loader: null,
        versionId: "1.21.1",
        createdAt: now - 172_800_000,
        source: null,
        lastPlayed: null,
        recommendedMemoryMb: null,
        overrides: { memory: null, javaId: null, launch: null, window: null, envVars: null },
        group: "建筑",
        link: null,
      },
      null,
      2,
    ),
  );

  return { sandboxDir, exePath, gameDir };
}

/** 轻量 W3C WebDriver 客户端（基于 Node 原生 fetch） */
class WebDriverClient {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
    this.sessionId = null;
  }

  async request(method, endpoint, body) {
    const res = await fetch(`${this.baseUrl}${endpoint}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await res.text();
    let json = {};
    try {
      json = text ? JSON.parse(text) : {};
    } catch {
      throw new Error(`WebDriver 返回非 JSON（HTTP ${res.status}）：${text.slice(0, 300)}`);
    }
    if (!res.ok || json.value?.error) {
      const err = json.value?.error ?? `HTTP ${res.status}`;
      const msg = json.value?.message ?? text.slice(0, 300);
      throw new Error(`WebDriver ${method} ${endpoint} 失败 [${err}]: ${msg}`);
    }
    return json.value;
  }

  async startSession(binaryPath) {
    const value = await this.request("POST", "/session", {
      capabilities: {
        alwaysMatch: {
          "tauri:options": {
            application: binaryPath,
          },
        },
      },
    });
    this.sessionId = value.sessionId;
    return value;
  }

  async closeSession() {
    if (!this.sessionId) return;
    const id = this.sessionId;
    this.sessionId = null;
    try {
      await this.request("DELETE", `/session/${id}`);
    } catch {
      // 忽略关闭阶段连接已断开的错误
    }
  }

  async execute(script, args = []) {
    return this.request("POST", `/session/${this.sessionId}/execute/sync`, { script, args });
  }

  async pressEscape() {
    await this.request("POST", `/session/${this.sessionId}/actions`, {
      actions: [
        {
          type: "key",
          id: "keyboard",
          actions: [
            { type: "keyDown", value: "\uE00C" },
            { type: "keyUp", value: "\uE00C" },
          ],
        },
      ],
    });
  }

  async waitFor(desc, predicateFn, timeoutMs = 15000, intervalMs = 150) {
    const deadline = Date.now() + timeoutMs;
    let lastError = null;
    while (Date.now() < deadline) {
      try {
        const result = await predicateFn();
        if (result) return result;
      } catch (err) {
        lastError = err;
      }
      await sleep(intervalMs);
    }
    let snapshot = "";
    try {
      const url = await this.execute("return location.pathname + location.search;");
      const text = (await this.bodyText()).replace(/\s+/g, " ").slice(0, 200);
      snapshot = ` | 当前页=${url} | 文本="${text}"`;
    } catch {
      // 忽略快照获取异常
    }
    throw new Error(
      `等待超时（${timeoutMs}ms）：${desc}${lastError ? `（最后异常：${lastError.message}）` : ""}${snapshot}`,
    );
  }

  async bodyText() {
    return (await this.execute("return document.body?.innerText ?? '';")) ?? "";
  }

  async clickButtonByText(text, { exact = false, scopeSelector = null } = {}) {
    return this.waitFor(`点击按钮「${text}」`, async () => {
      return this.execute(
        `
        const [needle, exact, scope] = arguments;
        const root = scope ? document.querySelector(scope) : document;
        if (!root) return false;
        const nodes = Array.from(root.querySelectorAll('button, label, [role="tab"], [role="menuitem"], [role="radio"], [role="option"]'));
        const visible = nodes.filter((el) => {
          const rect = el.getBoundingClientRect();
          const style = getComputedStyle(el);
          return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.opacity !== '0' && !el.disabled;
        });
        const match = visible.find((el) => {
          const t = (el.innerText || el.getAttribute('aria-label') || '').trim();
          return exact ? t === needle : t.includes(needle);
        });
        if (!match) return false;
        match.click();
        return true;
        `,
        [text, exact, scopeSelector],
      );
    });
  }

  async clickByAriaLabel(label) {
    return this.waitFor(`点击 aria-label="${label}"`, async () => {
      return this.execute(
        `
        const [needle] = arguments;
        const nodes = Array.from(document.querySelectorAll('a[aria-label], button[aria-label], [role="button"][aria-label]'));
        const match = nodes.find((el) => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && !el.disabled && (el.getAttribute('aria-label') || '').includes(needle);
        });
        if (!match) return false;
        match.click();
        return true;
        `,
        [label],
      );
    });
  }

  async setInputValue(selector, value) {
    return this.waitFor(`向输入框 ${selector} 填入「${value}」`, async () => {
      return this.execute(
        `
        const [sel, val] = arguments;
        const input = document.querySelector(sel);
        if (!input) return false;
        input.focus();
        const proto = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
        proto.set.call(input, val);
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
        `,
        [selector, value],
      );
    });
  }
}

async function waitForDriverReady(port, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/status`);
      if (res.ok) return;
    } catch {
      // 尚未监听
    }
    await sleep(150);
  }
  throw new Error(`tauri-driver 未在 ${timeoutMs}ms 内监听端口 ${port}`);
}

async function runSuite() {
  const tauriDriver = findExecutable("tauri-driver.exe", [DEFAULT_TAURI_DRIVER]);
  const edgeDriver = findExecutable("msedgedriver.exe", [DEFAULT_EDGE_DRIVER]);
  const appBinary = findAppBinary();

  if (!tauriDriver) {
    throw new Error(`找不到 tauri-driver.exe（可运行 cargo install tauri-driver --locked 安装到 D:\\dev\\cargo\\bin）`);
  }
  if (!edgeDriver) {
    throw new Error(`找不到 msedgedriver.exe（请下载与本机 WebView2 同版本的 Edge WebDriver 到 ${DEFAULT_EDGE_DRIVER}）`);
  }
  if (!appBinary) {
    throw new Error("找不到已编译的 Lumi 桌面可执行文件，请先运行 pnpm --dir app exec tauri build --debug --no-bundle");
  }

  const sandbox = createSandbox(appBinary);
  const driverPort = await getFreePort();
  const nativePort = await getFreePort();
  const results = [];

  console.log(`[E2E] 桌面二进制: ${appBinary}`);
  console.log(`[E2E] 隔离沙盒: ${sandbox.sandboxDir}`);
  console.log(`[E2E] tauri-driver: ${tauriDriver} (port=${driverPort}, native=${nativePort})`);
  console.log(`[E2E] msedgedriver: ${edgeDriver}`);

  const driverLogs = [];
  const driverProc = spawn(
    tauriDriver,
    [
      "--port",
      String(driverPort),
      "--native-port",
      String(nativePort),
      "--native-driver",
      edgeDriver,
    ],
    {
      env: {
        ...process.env,
        MCL_GAME_DIR: sandbox.gameDir,
        // 隔离环境强制使用简体中文首屏，确保断言确定性
        LANG: "zh_CN.UTF-8",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  driverProc.stdout?.on("data", (d) => driverLogs.push(d.toString()));
  driverProc.stderr?.on("data", (d) => driverLogs.push(d.toString()));

  const wd = new WebDriverClient(`http://127.0.0.1:${driverPort}`);

  async function step(name, fn) {
    const t0 = Date.now();
    process.stdout.write(`  → ${name} ... `);
    try {
      await fn();
      const ms = Date.now() - t0;
      console.log(`通过 (${ms}ms)`);
      results.push({ name, ok: true, ms });
    } catch (error) {
      const ms = Date.now() - t0;
      console.log(`失败 (${ms}ms)`);
      results.push({ name, ok: false, ms, error: error.message });
      throw error;
    }
  }

  try {
    await waitForDriverReady(driverPort);
    await wd.startSession(sandbox.exePath);

    // 确认前端界面已挂载且首屏（引导页或首页）已完成懒加载
    await wd.waitFor("前端根节点与首屏路由完成渲染", async () => {
      const body = await wd.bodyText();
      return (
        body.includes("选择启动器语言") ||
        body.includes("Choose launcher language") ||
        body.includes("星夜营地")
      );
    }, 25000);

    // 1. 首次运行引导主流程（语言 → 登录 → 导入 → 外观 → 准备好了 → 欢迎页 → 首页）
    await step("1. 首次运行引导主流程（语言 → 登录 → 导入 → 外观 → 准备好了 → 首页）", async () => {
      const text = await wd.bodyText();
      if (text.includes("选择启动器语言") || text.includes("Choose launcher language")) {
        // 第 1 步：语言 → 选「简体中文」→「下一步」
        await wd.clickButtonByText("简体中文");
        await sleep(150);
        await wd.clickButtonByText("下一步", { exact: true });

        // 第 2 步：登录（等待独有副标题出现，再点跳过）
        await wd.waitFor("引导进入登录步骤", async () => {
          return (await wd.bodyText()).includes("现在可以跳过");
        });
        await sleep(300);
        await wd.clickButtonByText("跳过", { exact: true, scopeSelector: "footer" });

        // 第 3 步：导入（取消勾选发现到的本机外部目录，确保沙盒纯净，再前进）
        await wd.waitFor("引导进入导入步骤", async () => {
          return (await wd.bodyText()).includes("导入已有游戏");
        });
        await sleep(350);
        const skippedSearch = await wd.execute(`
          const skipSearch = Array.from(document.querySelectorAll('footer button'))
            .find(b => !b.disabled && b.getBoundingClientRect().width > 0 && b.innerText.includes('先跳过查找'));
          if (skipSearch) { skipSearch.click(); return true; }
          return false;
        `);
        if (!skippedSearch) {
          // 先取消勾选所有默认选中的外部目录，等 React 提交状态后再点下一步
          await wd.execute(`
            for (const box of Array.from(document.querySelectorAll('[role="checkbox"][aria-checked="true"], input[type="checkbox"]:checked'))) {
              if (!box.disabled && box.getAttribute('aria-disabled') !== 'true') box.click();
            }
          `);
          await sleep(200);
          await wd.clickButtonByText("下一步", { exact: true, scopeSelector: "footer" });
        }

        // 第 4 步：外观 → 点「下一步」
        await wd.waitFor("引导进入外观步骤", async () => {
          return (await wd.bodyText()).includes("选择界面外观");
        });
        await sleep(300);
        await wd.clickButtonByText("下一步", { exact: true, scopeSelector: "footer" });

        // 第 5 步：准备好了 → 默认「先回首页」，点主按钮进入整窗欢迎页
        await wd.waitFor("引导进入准备好了步骤", async () => {
          return (await wd.bodyText()).includes("接下来做什么");
        });
        await sleep(350);
        await wd.waitFor("引导准备好了步骤前进到欢迎页", async () => {
          return wd.execute(`
            const btns = Array.from(document.querySelectorAll('footer button')).filter(b => !b.disabled && b.getBoundingClientRect().width > 0);
            const next = btns[btns.length - 1];
            if (!next || next.innerText.includes('上一步')) return false;
            next.click();
            return true;
          `);
        });

        // 第 6 步：整窗欢迎页 → 点「开始使用」进入首页
        await wd.waitFor("欢迎页展示「开始使用」", async () => {
          return (await wd.bodyText()).includes("欢迎使用 Lumi");
        });
        await sleep(300);
        await wd.clickButtonByText("开始使用");
      }

      // 断言首页已展示隔离样本世界「星夜营地」
      await wd.waitFor("首页呈现测试世界「星夜营地」", async () => {
        const body = await wd.bodyText();
        return body.includes("星夜营地") && body.includes("星夜生存");
      });
    });

    // 2. 首页「继续你的世界」与 D25a 正版登录拦截
    await step("2. 首页点击「继续」触发 D25a Microsoft 登录拦截弹窗", async () => {
      await wd.clickButtonByText("继续");
      await wd.waitFor("弹出登录 Minecraft 对话框", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("Minecraft") && (dialogText.includes("购买") || dialogText.includes("Microsoft"));
      });
      await wd.pressEscape();
      await wd.waitFor("登录弹窗按 Esc 关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 0;");
      });
    });

    // 3. 世界分享卡片 Canvas 渲染与深浅切换（D25e）
    await step("3. 世界分享卡片弹窗 Canvas 渲染与风格切换", async () => {
      await wd.clickByAriaLabel("更多");
      await wd.clickButtonByText("分享世界卡片");
      await wd.waitFor("分享卡片弹窗渲染出非空 Canvas", async () => {
        return wd.execute(`
          const dialog = document.querySelector('[role="dialog"]');
          if (!dialog) return false;
          const canvas = dialog.querySelector('canvas');
          return Boolean(canvas && canvas.width >= 600 && canvas.height >= 300);
        `);
      });
      await wd.clickButtonByText("浅色微光", { scopeSelector: '[role="dialog"]' });
      await sleep(200);
      await wd.pressEscape();
      await wd.waitFor("分享卡片弹窗关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 0;");
      });
    });

    // 4. 实例库检索、视图切换与实例详情全页签
    await step("4. 实例库检索、视图切换与实例详情（内容/世界/截图/日志/实例设置）", async () => {
      await wd.clickByAriaLabel("实例库");
      await wd.waitFor("实例库展示两个隔离实例", async () => {
        const body = await wd.bodyText();
        return body.includes("星夜生存") && body.includes("建筑沙盒");
      });

      // 搜索筛选
      await wd.setInputValue('input[type="search"], input[placeholder*="搜"]', "建筑");
      await wd.waitFor("搜索仅保留「建筑沙盒」", async () => {
        const body = await wd.bodyText();
        return body.includes("建筑沙盒") && !body.includes("星夜生存");
      });
      await wd.setInputValue('input[type="search"], input[placeholder*="搜"]', "");
      await wd.waitFor("清空搜索恢复全部实例", async () => {
        const body = await wd.bodyText();
        return body.includes("星夜生存") && body.includes("建筑沙盒");
      });

      // 点进「星夜生存」详情页
      await wd.waitFor("点击进入「星夜生存」实例详情", async () => {
        return wd.execute(`
          const candidates = Array.from(document.querySelectorAll('a, button, [role="button"], h2, h3, p'));
          const hit = candidates.find(el => el.innerText.trim() === '星夜生存' && el.getBoundingClientRect().width > 0);
          if (!hit) return false;
          hit.click();
          return true;
        `);
      });
      await wd.waitFor("进入实例详情页", async () => {
        const body = await wd.bodyText();
        return body.includes("星夜生存") && body.includes("内容") && body.includes("世界") && body.includes("截图") && body.includes("日志");
      });

      // 遍历核心页签：世界、截图、日志、内容
      for (const tab of ["世界", "截图", "日志", "内容"]) {
        await wd.clickButtonByText(tab);
        if (tab === "世界") {
          await wd.waitFor("世界页签展示「星夜营地」", async () => {
            return (await wd.bodyText()).includes("星夜营地");
          });
        }
      }

      // 打开实例设置弹窗，核对「通用」与「游戏（跟着全局）」栏目后按 Esc 关闭
      await wd.clickByAriaLabel("实例设置");
      await wd.waitFor("弹出实例设置窗口", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("实例设置") && dialogText.includes("通用");
      });
      await wd.clickButtonByText("游戏", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("实例设置「游戏」栏显示「使用全局设置」", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("使用全局设置") && dialogText.includes("内存");
      });
      await wd.pressEscape();
      await wd.waitFor("实例设置弹窗关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 0;");
      });
    });

    // 5. 发现内容页分类切换与面包屑后退
    await step("5. 发现内容页分类切换与标题栏导航后退", async () => {
      await wd.clickByAriaLabel("发现内容");
      await wd.waitFor("进入发现内容页", async () => {
        const body = await wd.bodyText();
        return body.includes("探索") && body.includes("整合包") && body.includes("资源包");
      });
      for (const kind of ["整合包", "资源包", "模组"]) {
        await wd.clickButtonByText(kind);
        await sleep(120);
      }
      await wd.clickByAriaLabel("后退");
      await wd.waitFor("后退回上一页（实例详情或上一分类）", async () => {
        const body = await wd.bodyText();
        return body.includes("星夜生存") || body.includes("探索资源包");
      });
    });

    // 6. 命令面板与关于页合规交互（D25f）
    await step("6. 命令面板直达「关于」页、隐私展开、许可弹窗叠层与第三方清单", async () => {
      await wd.clickByAriaLabel("搜索");
      await wd.waitFor("命令面板打开", async () => {
        return wd.execute("return Boolean(document.querySelector('input[placeholder*=\"搜实例\"]'));");
      });
      await wd.setInputValue('input[placeholder*="搜实例"]', "致谢");
      await sleep(150);
      await wd.clickButtonByText("关于");

      await wd.waitFor("进入设置「关于」页并展示 Kokoa77 开发者行、四大致谢与 GPL-3.0-only", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("Kokoa77") &&
          dialogText.includes("Lumi 的开发者") &&
          dialogText.includes("GPL-3.0-only") &&
          dialogText.includes("BMCLAPI") &&
          dialogText.includes("MCIM") &&
          dialogText.includes("Modrinth") &&
          dialogText.includes("CurseForge")
        );
      });

      // 展开账号数据与隐私
      await wd.clickButtonByText("展开说明", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("隐私边界详情展开", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("标准输入管道");
      });

      // 打开许可证弹窗并验证 Esc 只关最上层弹窗
      await wd.clickButtonByText("查看许可", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("叠层弹出开源许可证窗口", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 2;");
      });
      await wd.pressEscape();
      await wd.waitFor("Esc 仅关闭许可证弹窗，设置弹窗保持打开", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 1;");
      });

      // 打开第三方开源许可弹窗并切换分段
      await wd.clickButtonByText("查看清单", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("弹出第三方开源许可清单", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 2;");
      });
      await wd.clickButtonByText("前端界面");
      await wd.waitFor("切换到前端界面开源列表", async () => {
        const dialogs = await wd.execute(
          "return Array.from(document.querySelectorAll('[role=\"dialog\"]')).pop()?.innerText ?? '';",
        );
        return dialogs.includes("react-router") && dialogs.includes("motion");
      });
      await wd.pressEscape();
      await wd.waitFor("第三方许可弹窗关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 1;");
      });
    });

    // 7. 画面效果（流畅/精美）与中英双语实时切换
    await step("7. 设置「外观」切换流畅/精美效果与中英双语实时生效", async () => {
      await wd.clickButtonByText("外观", { scopeSelector: '[role="dialog"]' });
      await wd.clickButtonByText("流畅", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("流畅模式生效（data-fx=lite）", async () => {
        return wd.execute("return document.documentElement.dataset.fx === 'lite';");
      });
      await wd.clickButtonByText("精美", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("精美模式生效（data-fx=rich）", async () => {
        return wd.execute("return document.documentElement.dataset.fx === 'rich';");
      });

      // 切换语言到 English 并验证，随后切回简体中文
      await wd.waitFor("切换语言下拉框至 English", async () => {
        return wd.execute(`
          const dialog = document.querySelector('[role="dialog"]');
          if (!dialog) return false;
          const btn = Array.from(dialog.querySelectorAll('button')).find(b =>
            b.innerText.includes('简体中文') || b.innerText.includes('跟随系统')
          );
          if (!btn) return false;
          btn.click();
          return true;
        `);
      });
      await wd.clickButtonByText("English");
      await wd.waitFor("界面文案实时切换为英文", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("Settings") && dialogText.includes("Appearance") && dialogText.includes("About");
      });

      // 切回简体中文
      await wd.waitFor("切回简体中文", async () => {
        return wd.execute(`
          const dialog = document.querySelector('[role="dialog"]');
          if (!dialog) return false;
          const btn = Array.from(dialog.querySelectorAll('button')).find(b => b.innerText.includes('English'));
          if (!btn) return false;
          btn.click();
          return true;
        `);
      });
      await wd.clickButtonByText("简体中文");
      await wd.waitFor("界面文案恢复为简体中文", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("外观") && dialogText.includes("关于");
      });

      // D26a 验收：侧栏搜索设置直达「内容与下载」、检查代理/并发/网络诊断，再检查「存储与备份」与「游戏」启动行为
      await wd.setInputValue('[role="dialog"] input[placeholder*="搜索设置"]', "代理");
      await wd.waitFor("设置搜索命中「网络代理」", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("网络代理");
      });
      await wd.clickButtonByText("网络代理", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("跳转到「内容与下载」并展示并发数、代理与网络诊断", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("最大下载并发数") &&
          dialogText.includes("网络代理") &&
          dialogText.includes("连通性检测")
        );
      });

      await wd.clickButtonByText("存储与备份", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("「存储与备份」展示磁盘占用、清理缓存、备份存放目录与偏好设置导出/导入", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("总占用") &&
          dialogText.includes("清理缓存") &&
          dialogText.includes("备份存放目录") &&
          dialogText.includes("导出设置") &&
          dialogText.includes("导入设置")
        );
      });

      await wd.clickButtonByText("实例同步", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("「实例同步」展示总开关、黄金 6 项资产与预览并同步入口", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("启用本机跨实例同步") &&
          dialogText.includes("编辑") &&
          dialogText.includes("多人服务器列表") &&
          dialogText.includes("命令记录") &&
          dialogText.includes("创造模式物品栏") &&
          dialogText.includes("预览并同步")
        );
      });
      // 打开游戏设置 9 栏可视化编辑器并验证左侧分栏
      await wd.clickButtonByText("编辑", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("叠层弹出共享游戏设置 9 栏可视化编辑器", async () => {
        const dialogs = await wd.execute(
          "return Array.from(document.querySelectorAll('[role=\"dialog\"]')).pop()?.innerText ?? '';",
        );
        return (
          dialogs.includes("编辑同步的游戏设置") &&
          dialogs.includes("皮肤自定义") &&
          dialogs.includes("音乐和声音") &&
          dialogs.includes("按键绑定") &&
          dialogs.includes("无障碍") &&
          dialogs.includes("自定义设置")
        );
      });
      await wd.pressEscape();
      await wd.waitFor("共享游戏设置编辑器关闭，设置弹窗保持打开", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 1;");
      });

      await wd.clickButtonByText("预览并同步", { scopeSelector: '[role="dialog"]' });
      await wd.waitFor("叠层弹出本机跨实例同步预览窗口并列出隔离实例", async () => {
        const dialogs = await wd.execute(
          "return Array.from(document.querySelectorAll('[role=\"dialog\"]')).pop()?.innerText ?? '';",
        );
        return (
          dialogs.includes("本机跨实例同步预览") &&
          dialogs.includes("智能合并") &&
          dialogs.includes("星夜生存")
        );
      });
      await wd.pressEscape();
      await wd.waitFor("同步预览弹窗关闭，设置弹窗保持打开", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 1;");
      });

      await wd.clickButtonByText("游戏", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("「游戏」栏展示默认游戏语言、启动后窗口行为、最大化与高级选项折叠入口", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("默认游戏语言") &&
          dialogText.includes("保持显示") &&
          dialogText.includes("最小化") &&
          dialogText.includes("释放界面") &&
          dialogText.includes("最大化") &&
          dialogText.includes("高级选项")
        );
      });
      await wd.clickButtonByText("展开", { exact: true, scopeSelector: '[role="dialog"]' });
      await wd.waitFor("展开「高级选项」可见窗口标题、启动钩子与 Discord 活动状态", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return (
          dialogText.includes("自定义游戏窗口标题") &&
          dialogText.includes("启动前命令") &&
          dialogText.includes("退出后命令") &&
          dialogText.includes("Discord 活动状态")
        );
      });

      await wd.pressEscape();
      await wd.waitFor("设置弹窗关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 0;");
      });
    });

    // 8. 下载中心面板、下载历史与全局零崩溃验证
    await step("8. 下载中心面板、下载历史弹窗与零渲染崩溃核验", async () => {
      await wd.clickByAriaLabel("下载");
      await wd.clickButtonByText("查看下载历史");
      await wd.waitFor("下载历史弹窗打开", async () => {
        const dialogText = await wd.execute(
          "return document.querySelector('[role=\"dialog\"]')?.innerText ?? '';",
        );
        return dialogText.includes("正在进行") && dialogText.includes("历史记录");
      });
      await wd.pressEscape();
      await wd.waitFor("下载历史弹窗关闭", async () => {
        return wd.execute("return document.querySelectorAll('[role=\"dialog\"]').length === 0;");
      });

      const crashed = await wd.execute(
        "return document.body.innerText.includes('界面出了点问题') || document.body.innerText.includes('Something went wrong');",
      );
      if (crashed) {
        throw new Error("检测到界面触发了 ErrorBoundary 错误兜底页");
      }
    });

    console.log(`\n[E2E] 全部 ${results.length}/${results.length} 项桌面端到端主流程全部通过！`);
  } finally {
    await wd.closeSession();
    driverProc.kill();
    await sleep(600);
    if (!KEEP_SANDBOX) {
      for (let i = 0; i < 6; i++) {
        try {
          fs.rmSync(sandbox.sandboxDir, { recursive: true, force: true });
          break;
        } catch {
          await sleep(400);
        }
      }
    } else {
      console.log(`[E2E] 已按 --keep 保留沙盒目录：${sandbox.sandboxDir}`);
    }
    if (results.some((r) => !r.ok) && driverLogs.length > 0) {
      console.error("\n--- tauri-driver 日志摘录 ---\n" + driverLogs.join("").slice(-2000));
    }
  }
}

runSuite().catch((err) => {
  console.error(`\n[E2E] 测试失败：${err.message}`);
  process.exit(1);
});
