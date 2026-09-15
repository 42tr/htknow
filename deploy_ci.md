## GitHub Actions 发布缓存

- 推送 `master` 时构建两个架构的 release 二进制，保存 Rust 依赖缓存；不上传发布产物、不推送镜像、不创建 Release。
- 推送 `v*` tag 时复用默认分支的依赖缓存，构建二进制并发布。GitHub 不允许不同 tag 互读各自的 Actions 缓存，所以需要先让 master 的构建成功完成。首次构建、Rust 版本或依赖变化时仍可能需要重新编译。
- 手动运行在分支上会构建并推送 `sha-<短提交哈希>` 镜像；在 tag 上运行则使用该 tag 发布。
- Docker 构建缓存保存在 GHCR 的 `buildcache` tag，供不同版本复用运行时依赖层；该 tag 是缓存，不用于部署。缓存不写入阿里云 ACR。
- `Dockerfile.release.dockerignore` 将镜像上下文限制为 `dist/amd64/htknow` 和 `dist/arm64/htknow`。

验证加速效果时，先等 master 构建完成，再发布 tag；检查 `Cache Rust build` 是否恢复缓存，以及 Docker 安装依赖步骤是否显示 `CACHED`。对比 `Build release binary` 和 `Build and push multi-arch image` 的耗时。

## 配置 gitea 密钥
`DEPLOY_HOST` SSH HOST
`DEPLOY_PORT` SSH PORT
`DEPLOY_USER` SSH USER
`DEPLOY_PASSWORD` SSH PASSWORD
`DEPLOY_PATH` 部署路径
`DEPLOY_COMMAND` 部署命令
```shell
set -e

TAR_FILE="$DEPLOY_PATH/$ASSET_FILE"
COMPOSE_FILE="$DEPLOY_PATH/docker-compose.yaml"   # 或写绝对路径

EXTRACTED_TAR="${TAR_FILE%.gz}"
gzip -dc "$TAR_FILE" > "$EXTRACTED_TAR"
docker load -i "$EXTRACTED_TAR"
rm -f "$EXTRACTED_TAR"

if docker compose version >/dev/null 2>&1; then
  IMAGE_TAG="$SAFE_TAG" docker compose -f "$COMPOSE_FILE" up -d --remove-orphans
else
  IMAGE_TAG="$SAFE_TAG" docker-compose -f "$COMPOSE_FILE" up -d --remove-orphans
fi
```
