# 自托管同步服务

CocoBrowser 的跨设备同步**只支持自托管**。上游的托管账号层已经删除，客户端里没有
"登录账号即可同步"这条路 —— 你需要自己运行一个 `coco-sync` 服务和一份 S3 兼容存储。

本文覆盖 Docker Compose 部署。相关文件都在 `coco-sync/` 下：

| 文件 | 作用 |
|---|---|
| `docker-compose.selfhost.yml` | 主编排：`sync` 服务，可选 `minio` profile |
| `docker-compose.tls.yml` | 叠加文件，加上 Caddy 自动 HTTPS |
| `.env.example` | 全部环境变量，复制成 `.env` 后填写 |
| `Caddyfile.example` | 反代配置，复制成 `Caddyfile` |
| `Dockerfile` | 服务镜像，构建上下文是**仓库根目录** |

## 一条必须先理解的约束

服务端把 **预签名 S3 URL** 交给客户端，让客户端直接和对象存储通信。这些 URL 是用
`S3_ENDPOINT` 签出来的，而代码里**没有单独的"对外地址"设置**
（`coco-sync/src/sync/sync.service.ts` 里 `getSignedUrl` 直接用同一个 client）。

所以：

> `S3_ENDPOINT` 必须是**运行 CocoBrowser 的那台机器能访问到的地址**。

写 `http://minio:9000` 会让客户端拿到一个它解析不了的域名，同步表现为连接失败。这是自托管
MinIO 最常见的坑。

## 方案 A：外部 S3 / R2（推荐，最省事）

存储用 Cloudflare R2 或 AWS S3，只自己跑 `coco-sync` 一个容器。R2/S3 的 endpoint 本身就是
公网地址，上面那个坑自动不存在。

```bash
cd coco-sync
cp .env.example .env
```

填 `.env`：

```
SYNC_TOKEN=<openssl rand -hex 32 的输出>
S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
S3_ACCESS_KEY_ID=<R2 API token 的 access key>
S3_SECRET_ACCESS_KEY=<R2 API token 的 secret>
S3_BUCKET=coco-sync
S3_FORCE_PATH_STYLE=true
```

用 AWS S3 时把 `S3_FORCE_PATH_STYLE` 设为 `false`，并把 `S3_REGION` 改成桶所在区域。
bucket 不存在时服务会自己创建（`ensureBucketExists`）。

启动：

```bash
docker compose -f docker-compose.selfhost.yml up -d --build
```

默认只监听 `127.0.0.1:12342`。要让别的设备连上，前面必须有 TLS —— 见下面《加 HTTPS》。
`SYNC_TOKEN` 是唯一凭据，明文 HTTP 暴露到公网等于把它送出去。

## 方案 B'：部署到 NAS，设备都在局域网内

这是最简单的一种，比公网部署省掉整个 TLS 环节。NAS 用固定内网 IP（假设
`192.168.1.20`），所有设备都在同一局域网。

`.env`：

```
SYNC_TOKEN=<openssl rand -hex 32 的输出>
S3_ENDPOINT=http://192.168.1.20:8987
S3_ACCESS_KEY_ID=<自定义>
S3_SECRET_ACCESS_KEY=<自定义，至少 8 位>
S3_BUCKET=coco-sync
S3_FORCE_PATH_STYLE=true
SYNC_BIND=0.0.0.0
MINIO_BIND=0.0.0.0
```

两个 `BIND` 必须改成 `0.0.0.0`，否则只有 NAS 自己能连。`S3_ENDPOINT` 用 NAS 的内网 IP —— 
局域网内每台设备都能解析，前面说的预签名坑自然不存在。别用 `.local` 主机名，Windows 对
mDNS 的支持不稳定。

```bash
docker compose -f docker-compose.selfhost.yml --profile minio up -d --build
```

Synology / QNAP 的 Container Manager 支持直接导入 compose 文件，但镜像需要现场构建
（`build:` 段），部分机型的图形界面不支持构建。那就先在别的机器上
`docker build -t coco-sync:local -f coco-sync/Dockerfile .`，推到你自己的 registry 或用
`docker save` / `docker load`，再把 compose 里的 `build:` 换成 `image: coco-sync:local`。

镜像基于 `node:22-alpine` 和官方 `minio`，两者都有 arm64，ARM 架构的 NAS 可以直接用。

客户端里同步地址填 `http://192.168.1.20:12342` —— 代码不强制 HTTPS。局域网内 `SYNC_TOKEN`
是明文传输的，如果你的 Wi-Fi 不完全可信，还是套一层 TLS。

要在外网也能同步，就得让 NAS 的两个端口能从公网访问（DDNS + 端口转发，或者 Tailscale /
WireGuard 这类组网）。用组网方案时 `S3_ENDPOINT` 填 NAS 在虚拟网段里的地址，所有设备一致。

## 方案 B：全自托管，含 MinIO

加上 `minio` profile：

```bash
docker compose -f docker-compose.selfhost.yml --profile minio up -d --build
```

`.env` 里 `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` 同时用作 MinIO 的 root 账号密码。

`S3_ENDPOINT` 要填 MinIO 的**对外**地址。配合下面的 HTTPS 方案就是
`https://s3.your-domain.com`。同时把 `S3_HOST_ALIAS` 设成该域名的主机名部分：

```
S3_ENDPOINT=https://s3.your-domain.com
S3_HOST_ALIAS=s3.your-domain.com
```

`S3_HOST_ALIAS` 会通过 `extra_hosts` 把这个域名在 sync 容器内指回宿主机，这样服务端自己访问
存储时不依赖服务商的 hairpin NAT（很多 VPS 不支持容器绕公网 IP 回到自己）。

## 加 HTTPS

需要**两个** DNS 记录都指向这台服务器，因为客户端要分别访问 API 和对象存储：

- `sync.your-domain.com` → 同步 API
- `s3.your-domain.com` → 对象存储

```bash
cd coco-sync
cp Caddyfile.example Caddyfile
```

`.env` 里补上：

```
SYNC_DOMAIN=sync.your-domain.com
S3_DOMAIN=s3.your-domain.com
ACME_EMAIL=you@your-domain.com
```

启动（叠加 tls 文件）：

```bash
docker compose -f docker-compose.selfhost.yml -f docker-compose.tls.yml --profile minio up -d --build
```

Caddy 自动申请并续期证书，需要 80 端口可达做 ACME 校验。叠加文件会清掉 `sync` 和 `minio`
自己的宿主端口映射，全部流量走 Caddy。

只用方案 A（外部 S3）时，`s3.your-domain.com` 那段不需要，但 `Caddyfile` 里留着它会让 Caddy
为一个没有后端的域名申请证书 —— 把那一段删掉。

## 客户端配置（每台设备都要做）

界面上是两个地方，**第三样最容易漏**：

1. 设置 → 同步：填**服务地址**（`https://sync.your-domain.com`）和**令牌**（`SYNC_TOKEN`），
   点"测试连接"。
2. 设置 → 同步：设**端到端加密口令**。想用 `Encrypted` 模式必须设，而且**所有设备的口令必须
   完全一致**，否则下载下来解不开。
3. 每个配置单独选同步模式：`Disabled` / `Regular` / `Encrypted`。

## 同步了什么

两套机制：

- **浏览器配置目录**（Cookie、历史、Local Storage 等真实文件）走**内容哈希清单**，逐文件比对
  hash 与大小，只传变化部分。
- **配置元数据、代理、VPN、分组、扩展、扩展组**各是一个小 JSON 整体上传，冲突按
  `updated_at` **最后写入胜出**。

**Persona 会同步** —— 指纹种子、时区、语言、屏幕、UA 都在配置元数据里，所以同一个配置在两台
设备上指纹一致。这是跨设备同步的主要意义所在。

## 上传和拉取分别在什么时候发生

这决定了多设备轮流使用是否安全，务必看清：

| 时机 | 行为 | 代码位置 |
|---|---|---|
| **点击打开配置** | **先取锁，再与远端对账**，远端更新则下载完才启动 | `sync/launch_gate.rs` → `prepare_launch` |
| 配置运行中 | 每 3 分钟续锁一次 | `launch_gate.rs` → `start_lock_heartbeat` |
| 关闭配置 | 立即上传，上传结束后释放锁 | `scheduler.rs` → `release_launch_lock` |
| 启动应用 | 全量对账，远端更新则下载 | `sync/mod.rs` → `sync_all_enabled_profiles` |
| 应用运行中，远端发生变化 | SSE 订阅推送，触发该配置同步 | `sync/subscription.rs` → `/v1/objects/subscribe` |

打开配置会**同步等待**这次对账完成，所以配置多、文件多时点开会有几秒延迟 —— 这是在换设备后
拿到最新数据的代价。

锁在上传**结束之后**才释放，不是在浏览器关闭时释放。否则会留下一个窗口：另一台设备在这一两秒
内打开配置，拉到的是本机还在写的中间状态。

## 跨设备互斥

同一个配置同一时间只能在一台设备上打开。第二台设备点开会被拒绝，并提示是哪台设备正在使用
（用主机名标识）。

锁存在远端 `locks/profiles/{配置ID}.json`，有效期 10 分钟，运行期间每 3 分钟自动续期。设备崩溃
或断电没来得及释放时，最多 10 分钟后自动失效。不想等就点配置名旁边的锁图标 →「清除占用」；
如果那台设备其实还在运行，它会在几分钟内重新占用，所以这个操作不会造成静默的数据覆盖。

**这把锁是建议性的，不是互斥量。** 取锁是"先读再写"，服务端的对象接口没有 compare-and-swap，
所以两台设备在同一个往返时间内同时点开，可能都认为自己拿到了锁。它挡的是"忘了在另一台设备上
关掉"这种日常情况，不是刻意的竞争。

**离线时不阻塞。** 连不上同步服务器就无法取锁也无法对账，这时仍然允许打开 —— 否则断网就等于
不能用 —— 但会弹一条警告，说明该配置可能不是最新的、并且此刻拦不住第二台设备同时打开。看到这
条警告就该去检查同步服务器。

同时打开一旦真的发生，后果是：两边各写自己的本地文件，谁后关闭谁的版本上传成功，另一边这段
时间的 Cookie、登录态、历史全部丢失，且不会报错。配置元数据是 `updated_at` 最后写入胜出，浏览器
文件是内容哈希清单覆盖，两者都没有冲突检测。

## 跨设备的三个限制

**代理密码换设备解不开。** 密码用 Windows DPAPI 加密，密文绑定到原来那个 Windows 用户账户。
同步会把密文传过去，新设备解不开。**新设备必须重新输一遍代理密码。**

**跨操作系统只同步元数据。** `host_os` 不一致时只下载配置定义，不下载浏览器文件。Windows 和
macOS 之间同步不了登录态。

**内核不同步。** fingerprint-chromium 约 190 MB，每台设备各自下载并各自校验 SHA-256。

另外 Chromium 自己的 Cookie 数据库和 Login Data 也有设备绑定成分。`Encrypted` 模式下我们这层
AES-256-GCM 是可靠的，但 Chromium 内部那层加密不受本项目控制。

## 排查

服务端两个探针：

```bash
curl https://sync.your-domain.com/health   # 进程活着
curl https://sync.your-domain.com/readyz   # 加上 S3 可达、bucket 存在
```

`/readyz` 返回 503 就是 S3 那侧的问题（凭据、endpoint、bucket 权限）。

服务启动时会主动拒绝几种错误配置，日志里能直接看到原因：

| 现象 | 原因 |
|---|---|
| `SYNC_TOKEN is a known default or too short` | 令牌是占位符或短于 24 字符 |
| `Either SYNC_TOKEN or SYNC_JWT_PUBLIC_KEY must be set` | 两个都没设 |
| `Required environment variable S3_ENDPOINT is not set` | 缺 S3 必填项 |
| `S3 connection failed` | endpoint 不可达或凭据错误 |

客户端能连上但同步卡住、报存储错误，先怀疑 `S3_ENDPOINT` 是不是客户端解析不了的内网地址。

配置打不开、提示被某台设备占用，但那台设备明明已经关了：说明它上次没能正常释放锁。等 10 分钟
自动失效，或者点锁图标 →「清除占用」。

每次打开配置都弹"未校验同步服务器就打开了"：客户端连不上同步服务器。此时锁和对账都没生效，多
设备并用不安全，先按上面的探针查服务端。

## 不用 Docker 直接跑

```bash
cd coco-sync
pnpm install
pnpm build
node dist/main
```

环境变量同 `.env.example`。`PORT` 默认 12342。

## 服务端的历史残留

`src/auth/auth.guard.ts` 里还有 `SYNC_JWT_PUBLIC_KEY`、`BACKEND_INTERNAL_URL`、
`BACKEND_INTERNAL_KEY` 和团队作用域逻辑，那是上游托管账号方案的服务端部分。自托管不要设这些
变量 —— 不设时相关代码路径不会进入，鉴权走静态 `SYNC_TOKEN`。客户端侧对应的代码已经删除。
