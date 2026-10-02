# nasfind v0.2.0

新增 Rust 原生目录统计，日常使用统一为 `nasfind` 命令。

- `nasfind stats` 支持 Top N、指定索引、子目录及递归/非递归统计。
- `nasfind index update` 自动初始化新索引，并在更新后维护统计缓存。
- `stats.db` 同时保存两种计数，路径只存一份；旧版缓存自动迁移，不重建文件索引。
- 统计只查询数据库，不遍历索引目录；多索引重复路径会去重。
- 更新进度显示处理速度、耗时及估算剩余时间，首次建库不虚报 ETA。
- 移除旧版 `dircount` 脚本，目录统计统一使用 `nasfind stats`，不依赖 Python。
- 通用配置模板与本地 NAS 配置分离，Rust 单元测试移到 `tests/unit/`。

日常命令：

```bash
nasfind index update
nasfind stats -n10
nasfind stats --recursive false
```

发布流程构建 Linux x86_64、aarch64 静态程序。plocate 为外部依赖；群晖安装请参考 `SYNOLOGY.md`，依赖准备脚本仍需 Python 与网络。
