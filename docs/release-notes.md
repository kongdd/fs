# nasfind v0.2.0

为 NAS 文件名检索提供 plocate 的轻量 CLI 封装。

- 提供 Linux x86_64、aarch64 静态二进制 ZIP，无需 Rust 编译器。
- 群晖用户先运行 `uname -m` 选择架构，再按 `SYNOLOGY.md` 准备预编译依赖。
  `python3 setup-tools.py` 从官方 Alpine 软件源下载私有 plocate、GNU sort 和运行库；
  需要 Python 3.8+ 和网络。ZIP 未内置这些外部工具。
- 支持多个 DB；全局过滤作为默认，局部按字段覆盖，未定义继承，`[]` 禁用该项。
- 默认排除已指定语言的环境、依赖、Rust 编译输出、NAS 回收站与系统元数据。
- 支持 `nasfind -i -l 50 soil` 等省略 search 的操作。
- `index --folder PATH` 只扫描子目录并合并回主 DB；失败保留旧 DB。
- 提供 NAS 效率测试脚本 `benchmark.py`，记录建库、局部合并及查询 P50/P95。
- 普通 Linux 定时器每 48 小时更新；群晖请用 DSM 任务计划程序设定每两天更新。

局部合并需本地重建主 DB，会丢失目录复用缓存；含换行的文件名需要全量更新。
发布前在两种架构 Linux 上验证静态程序、依赖准备、索引与查询；尚未在你的 DSM 7.4
上实测，性能需要运行安装包中的基准脚本。当前不提供 Mac/Windows 原生程序。
