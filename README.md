## 启动 mineru
```shell
docker run -d --name mineru-api --restart unless-stopped --ipc host -p 10001:10001 -e MINERU_MODEL_SOURCE=local --ulimit memlock=-1 --ulimit stack=67108864 --gpus all alexsuntop/mineru:latest mineru-api --host 0.0.0.0 --port 10001
```

## 启用 etcd 配置
`cargo build --features etcd`
