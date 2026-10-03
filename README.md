# fs

跨平台文件名索引与检索工具。默认原生 Rust 引擎，无需 updatedb/plocate；支持 Everything 表达式、增量更新、目录排除、离线查询和目录统计。

## 使用

```sh
cargo build --release --bin fs
./target/release/fs init
# 编辑 ~/.config/fs/config.toml，设置扫描目录和数据库路径
./target/release/fs updatedb
./target/release/fs locate soil
./target/release/fs stats
```

数据库必须位于扫描目录之外。配置示例见 [examples/](examples/)；Windows 路径建议写成 `C:/data`。

配置查找顺序：`--config`、`FS_CONFIG`、旧 `NASFIND_CONFIG`、用户配置目录、系统配置目录。每个配置目录先找 `fs/config.toml`，再找旧 `nasfind/config.toml`；Windows 也查找 APPDATA。

`index`、`search` 是兼容别名；`fs soil` 等同于 `fs locate soil`。原生索引 schema v2 不变，可以继续使用同系统的 nasfind v0.3.x 数据库；旧 v1/plocate 数据库不自动转换。

## 常用命令

```sh
fs updatedb                       # 更新所有索引，自动初始化缺失数据库
fs updatedb research              # 只更新指定索引
fs updatedb --folder /data/project # 只更新子目录，保留其他条目
fs updatedb init                  # 只初始化缺失数据库
fs locate '<soil | rain> ext:nc !backup'
fs locate --files --ext nc --path /data/project -l 20 soil
fs locate --offset 20 --json soil
fs locate --locate -i -b soil      # 传统 locate 匹配语义
fs ignore add cache node_modules  # 查询时隐藏目录，不修改索引
fs ignore list
fs ignore rm cache
fs stats -n20
fs doctor
```

查询默认匹配文件名、忽略 ASCII 大小写；索引查询不访问扫描目录，只有 `--existing` 检查结果是否仍存在。`--dirs / --files` 根据文件名后缀推断类型，不读取文件元数据。`--mnt` 保留 NAS 挂载路径输出映射。

Unix 可显式使用 `fs updatedb --engine plocate`；此时需要外部 plocate/updatedb。Windows 仅支持原生数据库。Linux 私有工具安装脚本为 `scripts/tools/setup-plocate.py`，不会创建系统级扫描任务。

## 布局

```text
crates/
├── core/       # 配置、路径编码、匹配和共享索引格式
├── updatedb/   # 扫描、建库、增量更新；可选 plocate 后端
└── locate/     # 检索、过滤、统计及 fs CLI
scripts/
├── benchmarks/ # 基准测试及共用测量工具
├── release/    # 发布包生成
└── tools/      # 可选辅助工具
tests/
├── unit/       # core / updatedb / locate 单元测试
├── integration/# CLI 集成测试
└── portable.rs # 跨平台库测试
```

脚本用法见 [scripts/README.md](scripts/README.md)，测试见 [tests/README.md](tests/README.md)，索引设计见 [docs/updatedb/](docs/updatedb/)。

## 验证与发布

```sh
make check
make e2e
make package TARGET=aarch64-apple-darwin
```

CI 在 Ubuntu 26.04、macOS 26 和 Windows 验证；UI 作业暂时注释。推送与 workspace 版本一致的 `v*` 标签后，发布工作流构建并测试五个目标：Linux x86_64/ARM64、Apple Silicon、Windows x86_64/ARM64，附 SHA-256 校验文件。不提供 Intel Mac 包。

[Releases](https://github.com/kongdd/fs/releases) 中旧 v0.3.x 包仍使用 nasfind 名称；重构后的源码生成 `fs`。[安装说明](INSTALL.md)适用于新的 fs 包。
