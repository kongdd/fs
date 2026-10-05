# fs

跨平台文件名索引与检索工具。默认使用 Rust 引擎建库，数据库格式在各操作系统上相同。Linux 可显式改用 plocate。支持 Everything 表达式、增量更新、目录排除、离线查询和目录统计。

## 使用

在仓库根目录安装：`cargo install --path . --locked`。

```sh
cargo build --release --bin fs
./target/release/fs init
# 编辑 ~/.config/fs/config.toml，设置扫描目录和数据库路径
./target/release/fs updatedb --engine rust -j 8 # 跨平台；-j 为并行扫描线程数
./target/release/fs locate soil
./target/release/fs stats
```

路径统一用 `/`，包括 Windows 的 `C:/` 和网盘 `//server/share`，这样各系统生成的库可以互相查询。`updatedb` 在每台机器上扫描本机目录，写入该索引的 `database` 或 `update_database`。查询读取 `search_database`（通常是网盘上的库）；不指定索引名时，所有索引联合搜索，可用 `--path` 限制到某个网盘目录。配置示例见 [config/](config/)。

### 分离建库与查询配置

- [updatedb_nas.toml](config/updatedb_nas.toml)：NAS 建库，写入 `/volume1/CMIP6/.fs/`。
- [updatedb_mac.toml](config/updatedb_mac.toml)：Mac 本地建库，写入 `/Users/kongdd/.local/var/fs/mac.db`。
- [seach.toml](config/seach.toml)：Mac 联合查询 NAS 和本地库，格式从数据库头识别。

建库配置只需设置全局 `outdir`，每个索引按 `name` 生成 `<outdir>/<name>.db`；查询配置逐项填写完整的 `database` 路径。已有显式 `database`、`update_database`、`search_database` 配置仍兼容。两端均使用 Rust；NAS 使用新的数据库路径，不覆盖旧 plocate 库。按实际环境修改用户名和挂载路径；查询配置的 `root` 保持建库时的原始路径。文件名不会自动选择用途，须通过 `-c` 指定。

```sh
# NAS
fs -c config/updatedb_nas.toml updatedb -j 8
# Mac：先将 NAS /volume1/CMIP6 挂载到 /mnt/z
fs -c config/updatedb_mac.toml updatedb -j 8
fs -c config/seach.toml locate --mnt soil
```

配置查找顺序：`--config`、`FS_CONFIG`、旧 `NASFIND_CONFIG`、用户配置目录、系统配置目录。每个配置目录先找 `fs/config.toml`，再找旧 `nasfind/config.toml`；Windows 也查找 APPDATA。

`index`、`search` 是兼容别名；`fs soil` 等同于 `fs locate soil`。原生索引采用 schema v4：根目录只在 `meta.root` 保存一次，目录表和路径索引保存相对路径，并压缩仅供增量更新使用的 `directory_grams`。开发阶段仅支持当前 v4 原生格式，不兼容或自动迁移旧版原生数据库；旧库须使用新的数据库路径重新建库，程序不会自动覆盖。plocate 后端仍可显式选择，其数据库不自动转换。

目录变更检测使用秒级时间戳；Unix 保存 `dev、inode、mtime秒、ctime秒`（32 字节）。同一目录在一秒内发生的多次变化可能漏检，扫描前后的一致性检查也受此限制。

## 常用命令

```sh
fs updatedb                       # 默认 Rust，各系统数据库格式相同
fs updatedb --engine rust -j 8    # 原生引擎；-j 控制扫描线程，默认 1
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

默认引擎是 Rust，因此 Linux、macOS 和 Windows 建出的数据库可以互相复制使用。只有 Linux 能额外选择 plocate，且需要外部 plocate/updatedb；plocate 库不能在其他系统上查询。查询按已有 DB 格式自动选择后端，不受建库默认值影响。

默认建库引擎可随时切换，无需重编译：

```sh
fs config engine rust      # 写入当前配置；非 Linux 只能设这个
fs config engine plocate   # 仅 Linux
fs config engine           # 查看当前生效的默认引擎
export FS_ENGINE=rust      # 只影响当前进程，优先于配置文件
fs updatedb --engine rust  # 只影响这一次命令
```

优先级为 `--engine` > `FS_ENGINE` > 配置文件 `engine` > 内置默认 `rust`。切换默认值不会转换已有数据库，更新仍须选择与 DB 相符的后端。Linux 私有工具安装脚本为 `scripts/setup-plocate.py`，不会创建系统级扫描任务。

## 布局

```text
src/main.rs     # fs CLI 入口
crates/
├── core/       # 配置、路径编码、匹配和共享索引格式
├── updatedb/   # 扫描、建库、增量更新；可选 plocate 后端
└── locate/     # 检索、过滤、统计
scripts/
├── benchmarks/ # 基准测试及共用测量工具
├── release_package.py # 发布包生成
├── setup-plocate.py   # 可选 plocate 工具安装
└── index-and-count.sh # 索引更新与目录排行
tests/
├── unit/       # core / updatedb / locate 单元测试
├── integration/# CLI 集成测试
└── portable.rs # 跨平台库测试
```

索引实现按职责命名：`core/src/index_store.rs` 管理格式、读取与校验，`updatedb/src/index_builder.rs` 负责建库及增量更新，`locate/src/index_search.rs` 负责查询入口。

脚本用法见 [scripts/README.md](scripts/README.md)，测试见 [tests/README.md](tests/README.md)，索引设计见 [docs/updatedb/](docs/updatedb/)。

## 验证与发布

```sh
make check
make e2e
make package TARGET=aarch64-apple-darwin
```

CI 在 Ubuntu 26.04、macOS 26 和 Windows 验证；UI 作业暂时注释。推送与 workspace 版本一致的 `v*` 标签后，发布工作流构建并测试五个目标：Linux x86_64/ARM64、Apple Silicon、Windows x86_64/ARM64，附 SHA-256 校验文件。不提供 Intel Mac 包。

[Releases](https://github.com/kongdd/fs/releases) 中旧 v0.3.x 包仍使用 nasfind 名称；重构后的源码生成 `fs`。[安装说明](INSTALL.md)适用于新的 fs 包。
