# Brand & Trademark Policy · 品牌与名称政策

**English** · [简体中文](#简体中文)

---

## English

Lumi's source code is free software under `GPL-3.0-only`. You may study, change and share it. This policy is about something else: **the name and the look that tell players a build really comes from the Lumi project**. It explains the Section 7 additional terms in [LICENSE](LICENSE) in plain words; if the two ever disagree, [LICENSE](LICENSE) prevails.

We have this policy for one reason: counterfeit launchers bundled with malware or adware are common in the Minecraft community. Players should be able to trust that anything called "Lumi" is the real thing.

### What Is Protected

- **Names**: `Lumi`, `Lumi Launcher` and `微光` when used for a Minecraft launcher or a related product
- **Identifiers**: the application ID `com.lumilauncher.lumi` and the User-Agent `LumiLauncher/<version>`
- **Visual identity**: the Lumi app icon and logo (`app/src-tauri/icons/`, `app/src/assets/art/lumi-mark.png`), the original illustrations (`app/src/assets/art/`), the promotional banner (`docs/screenshots/social-preview.png`) and the author avatar
- **Credentials**: Lumi's Microsoft Entra Client ID, updater signing key and public key, and API keys issued to Lumi

### What You Can Do Without Asking

- Talk about Lumi: write reviews, tutorials and news, make videos and streams, and use screenshots of Lumi in them
- Use the name and the unmodified logo to **refer to** Lumi, for example to link to this repository or the official releases
- Say your project is "based on Lumi Launcher" or "a fork of Lumi Launcher", as long as it is clearly not presented as Lumi itself
- Share links to the official download pages listed below
- Create your own community group about Lumi, as long as its name and description make clear it is unofficial (for example "Lumi players' group (unofficial)")

### Redistributing Official Builds

You may mirror an **unmodified** official build if all of the following are true:

- The files are byte-for-byte identical to the official release (their SHA-256 matches `SHA256SUMS.txt` on the release page)
- They are not wrapped in another installer, downloader or archive that adds software, ads or changes
- You link to the official release page and state that Lumi itself is free and the Lumi project never charges for it

### Forks and Modified Builds

If you distribute a modified version, you must:

1. **Use your own name, application ID and icon.** Do not use `Lumi`, `Lumi Launcher`, `微光` or anything confusingly similar as your product name, and do not reuse `com.lumilauncher.lumi`
2. **Replace the brand assets** listed above with your own. They are licensed under `GPL-3.0-only` together with the code, but the Section 7 terms forbid using them in a way that implies your build is official Lumi
3. **Say clearly that your build is modified** and that it is not made, endorsed or supported by the Lumi project
4. **Do not reuse Lumi's credentials.** Register your own Microsoft Entra application, generate your own updater keys and request your own API keys, or leave them unset
5. **Keep the license and copyright notices**, and publish your source code under `GPL-3.0-only`, as the license requires

### What Is Not Allowed

- Presenting any build, website, account or group as official Lumi when it is not
- Using the Lumi names or icon in domain names, app store listings, social media accounts or paid ads in a way that could make people think they are official
- Charging money in Lumi's name, or making people think they must pay to get the official Lumi; bundling it with malware, adware, cryptocurrency miners or unrelated software

### Official Channels

Only these are run by the Lumi project:

| Channel | Link |
|---|---|
| Official website | <https://lumilauncher.pages.dev> |
| Source code & downloads | <https://github.com/kokoa-77/LumiLauncher> (official downloads only from [Releases](https://github.com/kokoa-77/LumiLauncher/releases) and the official website above) |
| Discord | <https://discord.gg/vrAQ7HwUCw> |
| QQ group | [Lumi Launcher 官方交流 1 群](https://qm.qq.com/q/vSHOPvOKt2) |
| QQ Channel | [Lumi Launcher](https://pd.qq.com/s/Nr15r3slq) |
| Developer on Bilibili | <https://space.bilibili.com/25681030> |
| Sponsorship (afdian) | <https://afdian.com/a/lumilauncher> |

Lumi will never ask for your Microsoft password, and never sells accounts, "cracked" builds or paid features. Sponsorship is only accepted on the afdian page above.

### Reporting a Counterfeit

If you find a fake "Lumi" download, website or group, please [open an issue](https://github.com/kokoa-77/LumiLauncher/issues/new/choose) or tell us on Discord or in the QQ group. If it is spreading malware, use [private vulnerability reporting](https://github.com/kokoa-77/LumiLauncher/security/advisories/new) instead.

### Minecraft

Lumi is an independent third-party launcher. It is **not an official Minecraft product** and is **not approved by or associated with Mojang Studios or Microsoft**. "Minecraft" is a trademark of Mojang Studios.

---

## 简体中文

Lumi 的源代码是自由软件，以 `GPL-3.0-only` 发布，你可以学习、修改和分享。这份政策管的是另一件事：**让玩家认得出「这真的是 Lumi 官方出的」的名字和样子**。它用通俗的说法解释 [LICENSE](LICENSE) 里的第 7 条附加条款；两者有出入时，以 [LICENSE](LICENSE) 为准。

之所以要有这份政策，只有一个原因：Minecraft 社区中冒名、捆绑病毒或广告的启动器转载很常见。玩家应该能放心：叫「Lumi」的就是真的 Lumi。

### 受保护的内容

- **名称**：用于 Minecraft 启动器或相关产品的 `Lumi`、`Lumi Launcher`、`微光`
- **标识**：应用 ID `com.lumilauncher.lumi`、User-Agent `LumiLauncher/<版本号>`
- **视觉形象**：Lumi 应用图标和 Logo（`app/src-tauri/icons/`、`app/src/assets/art/lumi-mark.png`）、原创插画（`app/src/assets/art/`）、宣传横幅（`docs/screenshots/social-preview.png`）和作者头像
- **凭据**：Lumi 的 Microsoft Entra Client ID、自动更新的签名私钥和公钥，以及发给 Lumi 的各类 API Key

### 不用问就可以做的

- 谈论 Lumi：写评测、教程、新闻，做视频、直播，并在其中使用 Lumi 的截图
- 用名字和未修改的 Logo **指代** Lumi，比如链接到本仓库或官方发布页
- 说你的项目「基于 Lumi Launcher」或「Lumi Launcher 的分支」，只要不让人误以为它就是 Lumi
- 分享下面列出的官方下载地址
- 自己建 Lumi 的玩家群，只要群名和介绍写清楚是非官方的（例如「Lumi 玩家交流群（非官方）」）

### 转载官方版本

满足以下全部条件时，可以转载**未经修改**的官方版本：

- 文件与官方发布的逐字节一致（SHA-256 和发布页的 `SHA256SUMS.txt` 对得上）
- 没有套上别的安装器、下载器或压缩包来夹带软件、广告或做任何改动
- 注明并链接官方发布页，并注明 Lumi 本身免费、官方不收取任何费用

### Fork 与修改版

分发修改版时必须：

1. **用自己的名称、应用 ID 和图标**：产品名不得使用 `Lumi`、`Lumi Launcher`、`微光` 或容易混淆的近似名称，不得沿用 `com.lumilauncher.lumi`
2. **换掉上面列出的品牌素材**：它们和代码一样以 `GPL-3.0-only` 发布，但第 7 条附加条款禁止用它们让人以为你的版本是官方 Lumi
3. **明确说明这是修改版**，并非由 Lumi 项目制作、认可或提供支持
4. **不得沿用 Lumi 的凭据**：自己注册 Microsoft Entra 应用、生成自己的更新密钥、申请自己的 API Key，或者留空
5. **保留许可证与版权声明**，并按许可证要求以 `GPL-3.0-only` 公开源代码

### 不允许的

- 把任何非官方的版本、网站、账号或群说成是官方 Lumi
- 在域名、应用商店上架、社交媒体账号或付费广告里使用 Lumi 的名称或图标，让人误以为是官方
- 以 Lumi 的名义收费，或让人以为付费才能得到官方版本；把它和病毒、广告软件、挖矿程序、无关软件捆绑在一起

### 官方渠道

只有以下渠道由 Lumi 项目运营：

| 渠道 | 链接 |
|---|---|
| 官方网站 | <https://lumilauncher.pages.dev/zh/> |
| 源代码与下载 | <https://github.com/kokoa-77/LumiLauncher>（官方下载仅通过上方官网与 [Releases](https://github.com/kokoa-77/LumiLauncher/releases) 提供） |
| Discord | <https://discord.gg/vrAQ7HwUCw> |
| QQ 群 | [Lumi Launcher 官方交流 1 群](https://qm.qq.com/q/vSHOPvOKt2) |
| QQ 频道 | [Lumi Launcher](https://pd.qq.com/s/Nr15r3slq) |
| 开发者 B 站 | <https://space.bilibili.com/25681030> |
| 赞助（爱发电） | <https://afdian.com/a/lumilauncher> |

Lumi 永远不会向你索要 Microsoft 密码，也从不出售账号、「破解版」或付费功能。赞助只通过上面的爱发电页面接受。

### 举报冒名版本

发现冒充「Lumi」的下载、网站或群，请[提交 Issue](https://github.com/kokoa-77/LumiLauncher/issues/new/choose)，或在 Discord、QQ 群里告诉我们。如果它在传播恶意程序，请改用[私密安全报告](https://github.com/kokoa-77/LumiLauncher/security/advisories/new)。

### 关于 Minecraft

Lumi 是独立的第三方启动器，**并非 Minecraft 官方产品**，**与 Mojang Studios 及 Microsoft 无隶属关系，亦未获得其认可**。「Minecraft」是 Mojang Studios 的商标。
