# Rust 实验索引（独立构建）

借鉴 plocate 的 32 文件分块、zstd 和全局块级倒排。主程序仍使用 updatedb/plocate；此 crate 不进入主程序 CI 或发布包。

当前格式为 **schema v2**。v1 SQLite/Rust 库不迁移、不覆盖；请使用新数据库路径。完整设计、限制与实测见 [块级索引](docs/chunk-index.md)。其余设计与 Linux 性能文档为 v1 历史记录。

```bash
cargo build --release --manifest-path rust-engine/Cargo.toml
cargo test --manifest-path rust-engine/Cargo.toml --all-targets
python3 rust-engine/tests/integration/test-native.py rust-engine/target/release/nasfind

rust-engine/target/release/nasfind -c /path/config.toml index update --engine rust
rust-engine/target/release/nasfind -c /path/config.toml search soil
```

实验 CLI 使用隔离的接入代码与统计适配器，共享配置和统计缓存模块，不等同于主程序全部新功能。库测试覆盖原生索引及共享匹配模块；二进制测试覆盖实验统计快路径与共享统计测试，CLI 行为由原生端到端测试覆盖。主程序 `src/stats.rs` 不改动。

## CMIP6 同轮核验：旧倍率不成立

[扫描细分与交替首建核验](docs/cmip6-scan-validation.md)：此前 Rust 48.15 s 与 plocate 8.33 s 的单次结果缓存条件不同，**不能解释为慢 5.8 倍**。现在使用 `examples/config_nas.toml` 的 cmip6 规则、隔离临时数据库，同轮交替首建各 6 次，先清点目录、不清系统缓存。

诊断轮仍有明显缓存升温（两者首建范围约 4–15 s）；接着关闭诊断再做各 6 次：Rust 首建中位数 **3.945 s**、plocate **4.440 s**，更新各 3 次中位数 **1.214 / 3.888 s**。这是预热后的完整 CLI 成绩，不代表冷盘或所有布局。两次路径集合与结果核验均通过，生产库未改动。

可选环境变量 `NASFIND_SCAN_PROFILE=1` 细分扫描前 metadata、枚举/类型判断/分配、排序、发送背压、接收等待、大目录合并，以及目录 SQL 和写入后 metadata；默认关闭，双线程计时不能相加。已知 d_type、提前剪枝和未变目录复用都已实现，不重复宣称为待采用优化。本轮不改块或扫描算法。

## 当前首建：减少统计缓存重扫

单个原生索引、绝对路径根目录且没有 `exclude_files` / `exclude_extensions` 时，按目录汇总 `blocks.n`，避免为统计解压全部文件名、拼接每条路径。根目录记录单独计入；空目录记录、直接/递归计数和源目录离线行为保持不变。有文件过滤、多索引去重或 plocate 后端时仍逐路径统计。

快路径在同一个只读事务中检查块计数、大小和目录引用，并验证根记录；它不是对所有压缩文件名的完整校验。统计缓存格式、指纹检查及事务写入不变，不需要重建或迁移 v2 索引。

同一 cug-hydro 目录，新旧 release 各 5 次交替首建，基线为上述扫描流水线版本：

| 指标 | 流水线＋逐路径统计 | 流水线＋目录计数统计 |
|---|---:|---:|
| 完整 CLI 首建中位数 | 2.940 s | **2.623 s** |
| 统计阶段中位数 | 0.770 s | 0.610 s |
| 最大峰值 RSS | 39.50 MiB | 39.33 MiB |
| 索引大小 | 21.89 MiB | 21.89 MiB |

**本轮完整首建耗时减少 10.8%**，统计阶段减少 20.8%；不可把不同轮次的中位数相减当作累计收益。首轮新版 4.161 s、旧版 3.571 s，仍有波动。未做 plocate 对照。

库 46 项与二进制 50 项测试（含重复覆盖）、Clippy 和原生 CLI 集成测试通过。独立语料的新旧统计缓存总数、每目录计数、排名、子树/直接计数和离线重建均一致；结果集合与真实源路径指纹核验通过。原始报告：[目录计数统计首建对照](docs/cug-hydro-init-count-stats.json.txt)，基线保存在 `target/benchmarks/nasfind-before-stats`。

## 前一步：扫描与写入流水线

不改变目录独立块和 schema v2。首建由一个扫描线程预读目录，主线程负责 SQLite、压缩与倒排；增量更新仍走串行路径。小目录在扫描端排序，大目录分批传输后在写入端汇总排序，保持原有遍历顺序和 block ID。

- 队列最多 8 批，每批最多 512 条，估算文件名及条目数据达到 64 KiB 就发送；这不是总 RSS 上限，当前大目录、待遍历路径、容器容量等仍占内存。
- 保留扫描前与写入后的 stamp 检查、FULL 同步及事务回滚。失败时先断开队列再等待扫描线程退出，避免阻塞或遗留后台扫描。
- 新版 `scan` 阶段统计主线程等待扫描结果和目录行处理的耗时，后台扫描与 `index` 重叠；不能把该阶段缩短理解为文件系统工作减少。

同一 cug-hydro 目录，18.26 万条，新旧 release 各 5 次交替首建，不清缓存、只读源目录，临时索引在扫描目录外：

| 指标 | 上一版串行扫描 | 当前流水线 |
|---|---:|---:|
| 完整 CLI 首建中位数 | 3.780 s | **2.792 s** |
| 最大峰值 RSS | 38.64 MiB | 39.27 MiB |
| 索引大小 | 21.89 MiB | 21.89 MiB |

**首建中位数减少 26.2%（1.35× 加速）**，包含统计缓存维护。保留不利样本：首轮新版 5.317 s、旧版 4.428 s；本轮有明显波动，不能保证每次或所有布局均提速。未继续进行 plocate 对照。

结果集合与源路径指纹核验通过；46 个库单元测试、Clippy 和原生 CLI 集成测试通过，独立临时语料的六张数据表 payload 完全一致，新旧二进制交叉更新 v2 库通过。原始样本与二进制 SHA-256：[流水线首建对照](docs/cug-hydro-init-read-ahead.json.txt)。基线保存在 `target/benchmarks/nasfind-before-scan`。

## 上一轮：gram 查找与缓冲复用（未见整体收益）

在已有流式倒排和延后创建索引的基础上：

- 首建用按需分配的两级 gram 表代替逐 gram 哈希；连续数组保存编码状态，分段落盘与最终 SQL 写入仍按 gram 排序。
- 跨块复用压缩输出、文件名后缀缓冲，跨目录复用未压缩文件名缓冲，减少临时分配。
- schema v2、增量倒排合并、FULL 同步和事务回滚保持不变，不修改查询规划。

两级表在 64 位平台固定约 512 KiB，每个实际使用的页增加 1 KiB；极端覆盖全部 gram 时，查找表约 64.5 MiB，另加编码状态和数据。原有 16 MiB 分段阈值仍只限制编码积累，不是总 RSS 上限。缓冲复用会保留已分配容量。

同一 cug-hydro 目录，新旧 release 各 5 次交替首建，完整 CLI 中位数 **3.992 → 4.134 s（慢 3.6%）**；index 阶段 **1.198 → 1.067 s（减少 10.9%）**，scan 为 1.499 → 1.667 s，finalize 为 0.343 → 0.388 s。各阶段独立中位数不能直接相加。峰值 RSS 38.95 → 38.64 MiB，索引均为 21.89 MiB。结果集合与源路径指纹核验通过，42 个库单元测试通过。

**未验证完整建库提速，不将局部阶段收益当作整体优化成功。** 原始数据：[新旧首建复测](docs/cug-hydro-init-paged.json.txt)。后续 plocate 对照按要求中止，没有可用对照结论。下节真实目录成绩是更早版本的历史记录。

## 真实目录实测

[cug-hydro NAS 实测与首建优化](docs/cug-hydro-performance.md)：18.26 万条，首建旧/新各 5 次交替对照，5.56→4.23 s（耗时减少 24%），index 阶段约减半。上一轮优化版本与 plocate 同轮对照，首建 4.16 vs 2.95 s，仍慢约 41%；短词和纯扩展名扫描的差距更大。只读源目录，临时索引位于扫描目录外，schema v2 保持兼容。

```bash
python3 rust-engine/scripts/benchmark-real.py \
  --root /volume1/CMIP6/GitHub/cug-hydro \
  --output rust-engine/docs/cug-hydro-benchmark.json.txt
# 无 plocate 时增加 --native-only；--parent 指定临时数据库所在目录。
```

## 合成语料性能复现

优化前先保存旧实验二进制，再构建新版。基准使用独立临时目录，不修改已有 NAS 索引。

```bash
python3 rust-engine/scripts/benchmark-init.py \
  --before target/benchmarks/nasfind-before --after rust-engine/target/release/nasfind \
  --files 100000 --runs 5 --output init.json
python3 rust-engine/scripts/benchmark-native.py \
  --before target/benchmarks/nasfind-before --nasfind rust-engine/target/release/nasfind \
  --files 100000 --runs 21 --warmup 3 --output queries.json
# 真实目录：benchmark-init.py 用 --root /volume1/CMIP6/GitHub/cug-hydro 替换 --files。
# 旧二进制须保存在可执行文件系统，群晖 /tmp 可能 noexec。
# 单目录布局：benchmark-init.py 增加 --per-directory 100000。
# 与 plocate 直接对照：benchmark-native.py 不传 --before（需安装外部工具）。
```
