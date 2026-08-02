# CocoBrowser

CocoBrowser 是本地优先的多环境浏览器管理器。应用名称、Tauri 标识及数据目录继续使用 `CocoBrowser`；调试版使用 `CocoBrowserDev`。

当前新环境提供两种 CloakBrowser 内核模式：

- `cloakbrowser-150`：最新 v150，需配置一个免费或付费 Key；免费 Key 限一个活动会话。
- `cloakbrowser-146`：固定 v146 兼容模式，不需要 Key。

旧 `fingerprint-chromium` 环境保留读取和启动兼容，但不再用于新建环境。应用封装层保持 AGPL-3.0 开源；CloakBrowser 内核二进制为上游专有软件，不随 CocoBrowser 分发。

详细操作与隐私边界见 [USER_GUIDE.md](USER_GUIDE.md) 和 [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md)。
