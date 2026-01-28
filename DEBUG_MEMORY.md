# Debug 镜像内存分析指南

## 构建 Debug 镜像

Debug 镜像包含 jemalloc 内存分析工具:
- `libjemalloc-dev` - jemalloc 开发库
- `perl` - jeprof 脚本依赖
- `graphviz` - 生成可视化图表
- `binutils` - 符号解析工具

### 构建命令

```bash
# 构建 debug 镜像
./build-docker.sh debug

# 构建 release 镜像 (不包含分析工具)
./build-docker.sh
```

## 导出 Heap Dump

### 方法 1: 直接获取 PDF 报告 (最简单 🚀)

```bash
# 一键生成并下载 PDF 报告
curl -H "x-user-id: 1" \
     -H "x-user-name: admin" \
     -H "x-role: admin" \
     -o heap_analysis.pdf \
     http://localhost:8080/api/v1/knowledge/system/heap/pdf

# 然后直接打开查看
open heap_analysis.pdf  # macOS
xdg-open heap_analysis.pdf  # Linux
```

### 方法 2: 通过 API 导出原始 heap 文件

```bash
# 导出当前内存快照
curl -H "x-user-id: 1" \
     -H "x-user-name: admin" \
     -H "x-role: admin" \
     -o htknow.heap.$(date +%s).heap \
     http://localhost:8080/api/v1/knowledge/system/heap

# 查看堆分析状态
curl -H "x-user-id: 1" \
     -H "x-user-name: admin" \
     -H "x-role: admin" \
     http://localhost:8080/api/v1/knowledge/system/heap/status | jq

# 查看内存占用
curl -H "x-user-id: 1" \
     -H "x-user-name: admin" \
     -H "x-role: admin" \
     http://localhost:8080/api/v1/knowledge/system/memory | jq
```

### 方法 3: 进入容器手动导出

```bash
# 进入容器
docker exec -it htknow bash

# 安装 jeprof (如果需要在容器内分析)
apt update && apt install -y google-perftools

# 使用 jeprof 生成报告
jeprof --show_bytes --text /app/htknow /path/to/heap.file
```

## 分析 Heap Dump

### 手动分析

```bash
# 安装 gperftools (macOS)
brew install gperftools

# 安装 gperftools (Linux)
apt-get install google-perftools

# 生成 PDF 报告
jeprof --show_bytes --pdf target/debug/htknow htknow.heap.*.heap > report.pdf

# 生成文本报告
jeprof --show_bytes --text target/debug/htknow htknow.heap.*.heap > report.txt

# 查看前 10 个内存分配点
jeprof --show_bytes --text target/debug/htknow htknow.heap.*.heap | head -20
```

## 采样率配置

当前配置在 `src/main.rs:6`:

```rust
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:10\0";
```

| lg_prof_sample | 采样间隔 | 适用场景 | 性能开销 |
|----------------|----------|----------|----------|
| 0 | 每次分配 | 详细分析 | 非常高 |
| **10** | 1 KB | **推荐** | 中等 |
| 15 | 32 KB | 轻量监控 | 较低 |
| 19 | 512 KB | 只看大块 | 很低 |

## 内存优化

如果发现内存占用过高,可以调整以下配置 (在 `docker-compose.yml`):

### 1. 降低 Tantivy 缓存

```yaml
- HTKNOW_TANTIVY_MEMORY_MB=25  # 默认 50
```

### 2. 减少数据库连接

```yaml
- HTKNOW_DB_MAX_CONNECTIONS=5  # 默认 10
```

### 3. 调整搜索结果数量

```yaml
- HTKNOW_SEARCH_LIMIT=5  # 默认 10
```

## 容器内分析工具

进入容器后可用的工具:

```bash
# 查看进程内存
htop

# 查看系统内存
free -h

# 查看磁盘占用
df -h

# 查看目录大小
du -sh /app/data/*

# 实时监控容器
docker stats htknow
```

## 常见问题

### Q: 为什么 heap dump 文件很小?

A: 采样率太高 (lg_prof_sample:19),降低到 10 可获得更详细的数据。

### Q: 容器内存 251MB 但 heap 只有 1.7MB?

A: 可能原因:
1. 采样率过高,未记录小内存分配
2. 向量数据库 (LanceDB) 占用大量内存
3. 全文索引 (Tantivy) 缓存
4. Tokio 运行时和其他系统内存

### Q: Debug 镜像可以用于生产吗?

A: 不推荐。Debug 镜像:
- 包含额外的分析工具 (~100MB)
- 使用 debug 二进制 (更大,更慢)
- 启用了详细日志 (RUST_LOG=debug)
- 启用了内存分析 (性能开销)

生产环境请使用 release 镜像:
```bash
./build-docker.sh
```

## 参考链接

- [jemalloc Documentation](http://jemalloc.net/)
- [jeprof Manual](https://github.com/jemalloc/jemalloc/wiki/Use-Case%3A-Heap-Profiling)
- [Rust jemalloc](https://github.com/tikv/jemallocator)
