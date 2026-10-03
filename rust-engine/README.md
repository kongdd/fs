# Rust 原生索引

独立的 Rust 建库与检索引擎，使用 32 文件分块、zstd 和块级 trigram 倒排。v0.3.0 起提供跨平台预编译包，无需 Rust 或 plocate。原 plocate 源码版保留，也不保证全部 CLI 新功能一致。

## 下载与使用

在 [Releases](https://github.com/kongdd/nasfind/releases/latest) 下载对应系统和架构的包，解压即可使用；具体步骤见 [安装说明](INSTALL.md)。Linux、macOS 提供 x86_64/aarch64，Windows 提供 x86_64/ARM64。所有平台默认原生引擎。

## 源码构建

```bash
cargo build --release --manifest-path rust-engine/Cargo.toml
cargo test --manifest-path rust-engine/Cargo.toml --all-targets
python3 rust-engine/tests/integration/test-native.py rust-engine/target/release/nasfind

rust-engine/target/release/nasfind -c /path/config.toml index update --engine rust
rust-engine/target/release/nasfind -c /path/config.toml search soil
```

配置沿用主程序，数据库须放在扫描目录外。当前为 **schema v2**，不迁移或覆盖 v1 Rust 库及 plocate 库，请使用新数据库路径。

## 平台

- Linux、macOS、Windows 默认 Rust，无需外部索引工具；Unix 可显式使用 `--engine plocate`。
- Windows 使用 `nasfind.exe`；`nasfind init` 生成 Windows 配置示例。路径建议写成 `C:/data`，索引和输出路径统一使用 `/`。
- Unix 文件名字节保持原样；Windows 使用无损 WTF-8，保留非配对 UTF-16 代理项。数据库不跨系统迁移。
- Windows 不支持 plocate 库或后端；macOS 可选 plocate，但须自行安装对应工具。
- CI 覆盖 Linux、macOS、Windows 的建库、更新、搜索、统计和离线查询。

## 实现与限制

- **代码共享**：库与 CLI 共用模块；匹配、Everything 表达式和 plocate 建库复用主程序实现，仅保留原生接入与统计适配。
- **首建流水线**：一个扫描线程预读，主线程负责 SQLite、压缩和倒排；更新仍串行，并复用未变目录条目。
- **有界队列**：容量 8 批，按 512 条或估算 64 KiB 分批；大目录在写入端汇总排序。这不是总 RSS 上限。
- **一致性**：保留扫描前后 stamp 检查、FULL 同步和事务回滚，不改变目录独立块布局。
- **统计快路径**：单个原生索引、绝对根路径、无文件名/扩展名排除时汇总 `blocks.n`；其他情况逐路径统计。快路径校验计数元数据与根记录，不完整校验所有压缩文件名。
- **gram 与缓冲**：首建使用按需分配的两级查找表并复用缓冲；极端全覆盖时查找表约 64.5 MiB，不含编码数据。分段阈值不限制总 RSS。

设置 `NASFIND_SCAN_PROFILE=1` 可查看扫描细分计时，默认关闭。扫描与写入重叠，不能相加各线程累计时间。

## 性能

CMIP6，同机同配置，隔离临时库，预热后关闭诊断、交替首建各 6 次，更新各 3 次；完整 CLI 中位数包含统计维护：

| 指标 | Rust | plocate |
|---|---:|---:|
| 首建 | 3.945 s | 4.440 s |
| 无变更更新 | 1.214 s | 3.888 s |
| 首建最大峰值 RSS | 46.66 MiB | 167.14 MiB |
| 索引大小 | 24.71 MiB | 6.86 MiB |

**仅说明本轮预热建库表现，不代表冷盘或查询追平。** 旧的 48.15 / 8.33 s 对照缓存条件不同，不能推导“慢 5.8 倍”。最新试验的提前过滤与路径缓冲未见收益，已撤回。

cug-hydro 分轮对照：扫描流水线使首建中位数减少 26.2%，目录计数统计使首建减少 10.8%；两级 gram 表与缓冲复用未见完整 CLI 提速。各轮基线不同，不能叠加收益。

## 详细记录与基准

- [索引设计与合成语料实测](docs/chunk-index.md)
- [CMIP6 扫描细分、交替对照与复现](docs/cmip6-scan-validation.md)
- cug-hydro 原始报告：[扫描流水线](docs/cug-hydro-init-read-ahead.json.txt)、[目录计数统计](docs/cug-hydro-init-count-stats.json.txt)、[gram 与缓冲](docs/cug-hydro-init-paged.json.txt)
- [早期 NAS 建库与查询对照](docs/cug-hydro-performance.md)；其他 v1 设计及 Linux 性能文档仅作历史参考。

基准脚本位于 `scripts/`，参数见 `--help`：`benchmark-init.py`（首建）、`benchmark-native.py`（查询/更新）、`benchmark-real.py`（真实目录）、`benchmark-config.py`（配置驱动的交替对照）。使用隔离临时库，不覆盖生产索引；旧二进制须保存在可执行文件系统，群晖 `/tmp` 可能为 noexec。
