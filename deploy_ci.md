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
tar -xzvf "$TAR_FILE" -C "$DEPLOY_PATH"
docker load -i "$EXTRACTED_TAR"
rm -f "$EXTRACTED_TAR"

if docker compose version >/dev/null 2>&1; then
  IMAGE_TAG="$SAFE_TAG" docker compose -f "$COMPOSE_FILE" up -d --remove-orphans
else
  IMAGE_TAG="$SAFE_TAG" docker-compose -f "$COMPOSE_FILE" up -d --remove-orphans
fi
```
