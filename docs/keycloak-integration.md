# Keycloak 内网接入与迁移

本次修改包含 htknow、CoAssist 后端及 CoAssist_web 的登录入口。只生成代码和部署材料；没有在开发环境部署 Keycloak、启动应用或迁移业务数据库。

## 地址与部署材料

默认在内网 DNS（或每台客户端的 hosts）把 `cm.internal` 指向部署主机：

| 服务 | 浏览器访问地址 |
|---|---|
| CoAssist | http://cm.internal:19080 |
| htknow | http://cm.internal:11000 |
| Keycloak | http://cm.internal:18080 |
| issuer | http://cm.internal:18080/realms/cm |

容器之间使用 `http://keycloak:8080/realms/cm` 访问 Token、证书和 introspection 端点，但验证的 issuer 始终为上表的公开地址。客户端不需要解析 Docker 服务名。部署材料沿用用户提供的 cm-docker-compose.yaml 的服务名、端口；不复制其中的凭证，不修改或部署那个远程文件。

`deploy/keycloak/compose.oidc.yml` 是叠加文件，增加 Keycloak 26.7.3 与独立 PostgreSQL 18。生成的 realm 文件保持 0600；部署时需让 Keycloak 容器 UID 1000 可读取该单个文件（例如在部署主机执行 `chown 1000:0 keycloak-generated/cm-realm.json`），不要放宽其他凭证文件权限。所有挂载路径相对于第一个 Compose 文件；把生成的 `keycloak-generated` 放在主 Compose 文件旁。三个业务镜像必须先使用本次源码重新构建，不能继续运行修改前的镜像。

## 登录与身份

- 两个应用都是 confidential OIDC 客户端，采用 Authorization Code + S256 PKCE、随机 state、nonce、浏览器绑定 Cookie。回调交易在换码前原子消费。
- CoAssist 验证 ID Token（RS256/issuer/audience/nonce），并对 Access Token 做 introspection。上游 Access/Refresh Token 使用 Fernet 加密存库。
- CoAssist 为现有前端提供随机、不透明、数据库可撤销的应用会话。通过 HttpOnly 同源 Cookie 交接，不在 URL 携带令牌；前端继续使用原有 Bearer 会话存储机制，下载与 WebSocket 也验证相同会话。浏览器不会得到 Keycloak Refresh Token。现有下载和 WebSocket 的 query 参数兼容路径仍可能携带应用会话（不是 Keycloak Token）；部署网关日志需对 `access_token`/`token` 参数脱敏，后续可改为一次性资源票据。
- CoAssist 会话最长 8 小时，Keycloak Access Token 自动刷新；每次验证检查本地用户激活状态与 Keycloak introspection。刷新使用 PostgreSQL 行锁，协调多 worker 的 Refresh Token 轮换。SQLite 仅用于隔离测试，不支持同等行锁并发语义。
- htknow 的浏览器会话为 HttpOnly Cookie；数据库仅保存短期 Access Token（不保存 Refresh Token），有效期 5 分钟。到期重新进入 SSO；Keycloak 会话仍有效时通常无需输入密码。Cookie 写请求检查 Origin；重新登录可能中断正在编辑/上传的页面，应在后续体验优化中补自动续期。
- 两边 API 分别以 `coassist-api`、`htknow-api` 的独立客户端凭证执行在线 introspection，满足 Keycloak 对 introspection 调用方的 audience 限制；登录客户端密钥与资源验证密钥分开。校验 active、issuer、audience、expiry。首期优先采用明确的撤销语义，不做正向结果缓存；因此 Keycloak 暂不可用时认证请求失败关闭，并增加每次请求的网络开销。HTTP 连接复用。后续可以根据实测负载评估带短缓存的本地 JWKS 验证。
- 退出仅撤销当前应用会话，不清除 Keycloak 全局 SSO；再次选择登录可能直接进入。全局退出/back-channel logout 尚未实现。
- CoAssist 打开 OIDC 后旧密码登录、注册、找回密码和旧 JWT 被拒绝。htknow 默认 `HTKNOW_AUTH_MODE=oidc`，缺少配置启动失败；`trusted_headers` 仅显式用于旧隔离开发环境，不得作为无效 Bearer 的回退。

## 用户调用与服务调用

`coassist` 通过 Keycloak Standard Token Exchange V2，用当前用户 Access Token 换取 `aud=htknow-api` 的令牌。交换请求使用可选 scope `htknow-access`；它负责增加目标 audience，`audience` 参数用于收窄受众。未配置 Legacy Token Exchange 或用户模拟。

`coassist-worker` 使用 client_credentials。只有此客户端且有 `htknow-api/kb-sync` 客户端角色的令牌，被映射为 `service:coassist`。该主体没有全局 admin 身份，只能按既有资源 ACL 操作自己拥有的库/文件。频道创建、后台上传、迁移及 ACL 同步通过它调用；代理用户请求必须有用户会话，不允许降级为服务调用。普通用户只有显式分配 `htknow-api/htknow-admin` 才具有全局管理员权限，CoAssist 超级用户不会自动获得该角色。

`KnowledgeApiClient` 统一替换出站身份凭证，禁止向配置以外的 origin 发送凭证；审计保留业务操作者，以及实际调用客户端 ID 和用户委托/服务调用模式，不记录令牌。代理始终清除客户端 Authorization、Cookie 和身份头，未登录直接 401。

## ACL 与数据归属

旧 CoAssist 整数用户 ID 保留，htknow 中对应字符串 ID 保留。两边通过显式 `(issuer, subject) -> local user ID` 表绑定，不按用户名或邮箱自动认领。

频道库及已有子库的 owner 转移为 `service:coassist`，其成员权限由 CoAssist 原子同步：owner/admin 为该库 admin，member 为 editor，归档后非 owner 为 viewer。同步只接受服务主体且仅操作它拥有的库；支持空列表、移除旧成员、拒绝旧版本。频道库禁止公开，普通权限编辑接口不能覆盖频道 ACL；受管库的新增子库和移动层级操作被拒绝，避免子库脱离成员权限。已有子库随频道根库同步，迁移时拒绝互相重叠的频道库树。文件读取和列表也按所属 KB 的 ACL 检查，退出成员不再通过文件所有权绕过。

同步由成员变化触发，另有每轮结束后等待 30 秒的全量对账。**这不是即时撤权的强一致保证**：上游不可用或对账积压时，htknow 已有 ACL 可能暂时滞后。同步失败记录日志并由下轮修复；严格即时撤权需要增加授权租约/集中实时判定。部署后必须先完成对账，再开放用户入口。

## 部署主机上的迁移步骤（本次未执行）

1. 备份 CoAssist 数据库、htknow 数据目录及已有 Compose 文件。进入维护窗口，停止旧版业务写入。
2. 在 CoAssist 运行环境执行只读导出：

   ```sh
   python scripts/oidc_accounts.py export /secure/accounts.json
   ```

3. 在可以运行 Python 的位置生成离线导入包（不访问任何服务）：

   ```sh
   python /path/to/htknow/deploy/keycloak/generate.py /secure/accounts.json \
     --host cm.internal --output /path/to/compose/keycloak-generated \
     --htknow-admin 1
   ```

   `--htknow-admin` 为明确指定的已有用户 ID，可多次传入，不应盲目使用示例中的 1。不传时不创建 htknow 全局管理员。原 CoAssist 用户名允许重复，生成的登录名采用 `coassist-<ID>`，原姓名保留为显示名。每人分配不同临时密码，首次登录强制修改。`initial-passwords.json`、realm 文件与 env 文件含敏感信息，生成目录 0700、文件 0600，不提交版本库。

4. 人工核对 `bindings.json` 和 `channel_kb_ids`。脚本仅把导出的 CoAssist ID 映射为同值的 htknow 字符串 ID。原 htknow 单独使用的 `user1` 等非 CoAssist ID 不会自动认领；如果存在此类数据，应先明确所属用户，再单独审查迁移文件/库/ACL 的所有权。不要按姓名猜测归属。
5. 使用更新后的 CoAssist 代码应用绑定（显式写入两个数据库）：

   ```sh
   python scripts/oidc_accounts.py apply /secure/bindings.json \
     --htknow-db /mounted/htknow_data/app.sqlite
   ```

   **SQLite 文件路径要替换为实际 DATABASE_URL 指向的文件**。必须是现有文件，脚本不会新建空业务库。该步骤转移频道库 owner、清空旧频道 ACL，并保留非频道库权限。跨数据库不是分布式事务，失败后可以使用同一绑定包重跑；维护窗口内完成。脚本不启动应用，也不修改旧密码字段；旧密码入口由 OIDC 开关关闭。
6. 构建三份更新后的业务镜像，放置生成包，并在部署主机合并配置：

   ```sh
   docker compose --env-file keycloak-generated/oidc.env \
     -f cm-docker-compose.yaml -f /path/to/compose.oidc.yml config --quiet
   ```

   检查通过后才在部署主机启动。Keycloak 启动导入只适合首次创建 realm；已存在 `cm` realm 时不会自动覆盖它。不要重新生成随机客户端密钥而保留旧 realm，否则两边凭证会不一致。后续新增用户需在 Keycloak 创建账号并显式登记两边身份映射。
7. 等待 Keycloak 数据库就绪、业务初始化新增会话表以及频道 ACL 对账成功；完成下述验收后开放入口。账号禁用在 Keycloak 执行，本地 CoAssist 禁用也会阻止其会话并在对账时撤销频道授权。

HTTP 方案明确设置 realm `sslRequired=none`，只用于当前内网联调设计。正式改 HTTPS 时同步修改 issuer、回调、公开 URL、现有身份映射与 Cookie Secure，配置受信任证书；代理到 Keycloak 可继续 HTTP。不要只改浏览器 URL。

## 本地验证与范围

两个 Vue 前端生产构建通过。CoAssist 的鉴权、代理、ACL 同步、回调重放、旧入口禁用和迁移测试通过；迁移测试仅使用临时数据库。离线生成器测试以及叠加用户提供 Compose 后的 `config --quiet` 检查通过，未启动容器。

htknow 的独立鉴权测试直接编译生产 `auth_user`、`oidc` 和 `kb_acl` 模块，覆盖伪造身份头、错误令牌声明、服务身份隔离、ACL 撤销及退出成员的文件读取；运行方式见 `tests/auth/README.md`。完整库测试构建在 Lance 依赖阶段两次被终止，因此使用此独立测试包，并以 `cargo check --lib --bin htknow` 检查主程序集成。没有在当前环境执行真实 Keycloak 登录或 Token Exchange，以下联调验收仍需在部署主机完成。

## 必须在部署主机完成的验收

- 两边登录、首次改密、从 CoAssist 打开 htknow 的 SSO、应用本地退出。
- 回调错误 state/nonce、重放授权码均失败。
- 未登录、旧 JWT、伪造 x-role、错误 audience 均失败。
- 两个频道管理员互相访问私有库被拒绝；服务 token 不能访问系统管理员接口。
- 成员移除、用户禁用、归档、清空成员后，下载/搜索/图谱/导出和列表遵循新 ACL。
- Keycloak 暂停、恢复、签名密钥轮换、会话到期和并发刷新。
- 验证现有上传、流式搜索、下载及 WebSocket。当前代理的缓冲响应行为未在此改造为流式代理。

协议依据：[Keycloak Token Exchange](https://www.keycloak.org/securing-apps/token-exchange)、[反向代理](https://www.keycloak.org/server/reverseproxy)。
