# CocoBrowser — Local Fingerprint Chromium Build

这是一个面向 Windows x64、本地自用的多配置指纹浏览器管理器。它是 [Donut Browser](https://github.com/zhom/donutbrowser) v0.28.2 的派生作品，沿用其界面与配置管理能力，默认内核改为 [adryfish/fingerprint-chromium](https://github.com/adryfish/fingerprint-chromium)，不依赖任何账号、订阅或云代理。

依照 AGPL-3.0，本项目与上游采用相同许可。派生关系、修改范围与上游版权归属见 [NOTICE](NOTICE)；许可全文见 [LICENSE](LICENSE)。

> 指纹伪装只能降低配置之间的关联性，不能保证绕过网站风控、验证码或反滥用系统。请仅在你有权操作的账号、站点与测试环境中使用。

## 当前能力

- 独立配置目录、Cookie、扩展、代理和 Persona
- 固定审计的 Fingerprint Chromium `148.0.7778.215` 内核
- 自动一致的 Windows Persona，以及时区、语言、窗口、CPU 与 UA 高级配置
- HTTP、HTTPS、SOCKS4、SOCKS5 和 WireGuard 配置
- 出口 IP 与 Persona 地理一致性校验
- 单次及多轮指纹稳定性审计
- Chrome、Edge、Chromium 本地配置导入与首次启动清洗
- 加密 Cookie 导入导出；扩展包边界、哈希和权限审查
- 默认关闭的本地 REST API、MCP、无头模式与 JavaScript 执行
- `.portable` 标记驱动的便携数据目录

## 安全默认值

- 新配置只使用 Fingerprint Chromium；旧 Wayfern 配置仅为兼容历史数据保留。
- 内核从固定 HTTPS 地址下载，执行前校验嵌入清单中的大小与 SHA-256。
- 应用自更新入口已关闭，因为当前构建没有独立代码签名和签名发布清单。升级需手动校验发布包。
- API 与 MCP 默认关闭；启用后只监听回环地址并要求随机令牌。
- 无头自动化和 MCP JavaScript 执行分别单独授权，默认关闭。
- Cookie 只能导出为带密码的 `.cocookies` 文件。
- 代理密码使用 Windows DPAPI 或环境变量引用保存，不写入 Persona 签名与日志。

完整边界与残余风险见 [SECURITY_MODEL.md](SECURITY_MODEL.md) 和 [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md)。

## 使用

发布目录中的便携版本可直接运行：

1. 解压到普通可写目录，不要从 ZIP 内直接启动。
2. 运行 `Coco.exe`。
3. 打开“内核”，下载并校验固定版本的 Fingerprint Chromium。
4. 新建配置，按需设置代理，然后先执行指纹审计。

更完整的操作步骤见 [USER_GUIDE.md](USER_GUIDE.md)。

## 本地开发

要求：Windows x64、Node.js 24、pnpm 11、Rust stable 1.97 或更高，以及可用的 MSVC C++ 构建工具。

~~~powershell
pnpm install --frozen-lockfile
pnpm lint
pnpm test
pnpm build
cd src-tauri
cargo build --release --bins
~~~

`pnpm tauri dev` 用于交互开发。首次完整构建可能需要较长时间。

## 数据位置

- 便携模式：可执行文件旁存在 `.portable` 时，数据写入同目录下的 `data`、`cache` 与 `logs`。
- 隔离测试：设置 `COCOBROWSER_DATA_ROOT` 后，全部运行数据写入该目录。
- 普通安装：使用 Windows 用户的本地应用数据与缓存目录。

请备份完整数据目录；不要只复制单个配置文件。Cookie、浏览器数据库和 DPAPI 密文可能与 Windows 用户上下文绑定。

## 上游与许可

本项目基于 [zhom/cocobrowser](https://github.com/zhom/cocobrowser)，浏览器内核来自 [adryfish/fingerprint-chromium](https://github.com/adryfish/fingerprint-chromium)。代码继续遵循仓库中的 [AGPL-3.0 License](LICENSE)。第三方组件适用其各自许可。
