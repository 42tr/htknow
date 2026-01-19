## 启动 mineru
```shell
docker run -d --name mineru-api --restart unless-stopped --ipc host -p 10001:10001 -e MINERU_MODEL_SOURCE=local --ulimit memlock=-1 --ulimit stack=67108864 --gpus all alexsuntop/mineru:latest mineru-api --host 0.0.0.0 --port 10001
```

## 启用 etcd 配置
`cargo build --features etcd`

## 日志级别（RUST_LOG）
默认日志级别为 `info`，可通过环境变量覆盖：
```shell
RUST_LOG=debug ./htknow
RUST_LOG=warn,htknow::search=debug ./htknow
```
