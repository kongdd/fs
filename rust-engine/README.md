# Rust 实验索引（独立构建）

借鉴 plocate 的 32 文件分块、zstd 和全局块级倒排。主程序仍使用 updatedb/plocate；此 crate 不进入主程序 CI 或发布包。

当前格式为 **schema v2**。v1 SQLite/Rust 库不迁移、不覆盖；请使用新数据库路径。完整设计、限制与实测见 [块级索引](docs/chunk-index.md)。其余设计与 Linux 性能文档为 v1 历史记录。

```bash
cargo build --release --manifest-path rust-engine/Cargo.toml
cargo test --manifest-path rust-engine/Cargo.toml --lib
python3 rust-engine/tests/integration/test-native.py rust-engine/target/release/nasfind

rust-engine/target/release/nasfind -c /path/config.toml index update --engine rust
rust-engine/target/release/nasfind -c /path/config.toml search soil
```

实验 CLI 使用隔离的旧接入代码与共享配置/统计模块，不等同于主程序全部新功能。库测试覆盖原生索引及共享匹配模块，CLI 行为由原生端到端测试覆盖。

## 真实目录实测

[cug-hydro NAS 实测](docs/cug-hydro-performance.md)：18.26 万条，包含建库、无变更更新、查询分位数、RSS 和 plocate 同机对照。只读源目录，临时索引位于扫描目录外。

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
  --before /tmp/nasfind-before --after rust-engine/target/release/nasfind \
  --files 100000 --runs 5 --output init.json
python3 rust-engine/scripts/benchmark-native.py \
  --before /tmp/nasfind-before --nasfind rust-engine/target/release/nasfind \
  --files 100000 --runs 21 --warmup 3 --output queries.json
# 单目录布局：benchmark-init.py 增加 --per-directory 100000。
# 与 plocate 直接对照：benchmark-native.py 不传 --before（需安装外部工具）。
```
