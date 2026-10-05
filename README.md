# fs

[![CI](https://github.com/kongdd/fs/actions/workflows/ci.yml/badge.svg)](https://github.com/kongdd/fs/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/kongdd/fs/graph/badge.svg)](https://codecov.io/gh/kongdd/fs)

跨平台文件名索引与检索工具：先建立索引，再快速搜索，无需每次遍历磁盘。适合本地目录、NAS 和挂载网盘，索引建立后也能离线查询。

- **跨平台**：默认 Rust 引擎，支持 Linux、macOS 和 Windows，数据库可跨系统使用。
- **灵活检索**：支持 Everything 表达式、通配符、正则，以及目录、扩展名过滤。
- **增量更新**：更新整个索引或指定子目录，可排除依赖、缓存等无关文件。
- **目录统计**：查看索引中的文件数量和目录排行。

Linux 还可显式选择 plocate 后端；默认不依赖外部索引工具。

## 安装与快速上手

在仓库根目录安装：

```sh
cargo install --path . --locked
fs init
# 编辑 ~/.config/fs/config.toml，设置扫描目录和数据库路径
fs updatedb -j 8
fs locate soil
fs stats
```

预编译包见 [Releases](https://github.com/kongdd/fs/releases)，安装步骤见 [INSTALL.md](INSTALL.md)。

## 常用用法

```sh
# 建库与更新
fs updatedb                       # 更新所有索引，默认 Rust 引擎
fs updatedb --engine rust -j 8     # 显式选择引擎和扫描线程数
fs updatedb init                  # 只初始化缺失数据库
fs updatedb research              # 只更新指定索引
fs updatedb --folder /data/project # 只更新子目录

# 搜索：fs soil 等同于 fs locate soil
fs soil
fs locate '*.nc'
fs locate '<soil | rain> ext:nc !backup'
fs locate --regex '^soil[.]nc$' --basename
fs locate --files --ext nc --path /data/project -n20 soil
fs locate --offset 20 --json soil
fs locate --locate -i -b soil      # 传统 locate 匹配语义

# 查询时隐藏目录，不修改索引
fs ignore add cache node_modules
fs ignore list
fs ignore rm cache

# 统计与诊断
fs stats -n20
fs doctor
```

查询默认匹配文件名、忽略 ASCII 大小写；只有 `--existing` 会检查文件是否仍存在。路径统一使用 `/`，Windows 也可使用 `C:/` 和 `//server/share`。

### 切换建库引擎

```sh
fs config engine rust      # 保存默认引擎
fs config engine plocate   # 仅 Linux，需安装 plocate/updatedb
fs config engine           # 查看当前默认引擎
export FS_ENGINE=rust      # 仅对当前进程环境生效
```

`--engine` 可临时覆盖默认设置；切换引擎不会转换已有数据库。

### 记住查询与建库配置

路径保存为绝对路径。之后 `locate`、`stats`、`ignore` 使用 locate 配置，`updatedb` 使用 updatedb 配置。`-c` 与 `FS_CONFIG` 仍优先。

```sh
fs config set locate search.toml
fs config set updatedb updatedb.yaml
fs config list
```

### NAS 建库，Mac 查询

建库与查询可以使用不同配置：NAS 扫描文件并保存数据库，Mac 通过挂载目录读取数据库。

```sh
# NAS 建库（在仓库根目录运行）
alias update_nas='fs -c config/updatedb_nas.toml updatedb -j 8'
update_nas

# Mac 本地建库，再联合查询 NAS 和本地索引
fs -c config/updatedb_mac.toml updatedb -j 8
fs -c config/seach.toml locate --mnt soil
# 或先保存配置，以后可省略 -c
fs config set locate config/seach.toml
fs config set updatedb config/updatedb_nas.toml
```

使用前须修改 [config/](config/) 中的用户名、扫描目录和数据库路径，并挂载 NAS。查询配置的 `root` 保持建库时的原始路径；`--mnt` 将搜索结果转换为本机挂载路径。

## 仓库内容

- `crates/main.rs`：命令行入口。
- `crates/core/`：配置、路径匹配和共享索引格式。
- `crates/updatedb/`：扫描、建库和增量更新。
- `crates/locate/`：检索、过滤和统计。
- `config/`、`scripts/`、`tests/`：配置示例、辅助脚本和测试。

详细配置与限制见 [使用参考](docs/usage.md)，开发验证见 [测试说明](tests/README.md)，辅助工具见 [脚本说明](scripts/README.md)。
