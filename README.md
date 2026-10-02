# nasfind

NAS 文件名搜索工具，基于 **plocate**。支持多个索引、目录排除和更新进度；`nasfind stats` 可快速查看哪些目录包含最多的索引条目。

## 安装与配置（只需一次）

下载并解压 [Releases](https://github.com/kongdd/nasfind/releases) 中的 Linux 发布包，需要安装 plocate，日常搜索和统计不依赖 Python。群晖用户参考[安装说明](docs/synology.md)。

在解压目录中安装到用户目录，无需修改系统目录：

```bash
PREFIX="$HOME/.local" DATADIR="$HOME/.config/nasfind" ./install.sh
```

`~/.bashrc` 中配置命令路径和共用配置：

```bash
# 直接使用 nasfind 命令
export PATH="$HOME/.local/bin:$PATH"
# 共用默认配置，不必每次输入 --config
export NASFIND_CONFIG="$HOME/.config/nasfind/config.toml"
```

首次使用执行 `nasfind init`，然后编辑 `~/.config/nasfind/config.toml`，设置要索引的目录及数据库位置。数据库应放在扫描目录之外。已有配置不必重新初始化。

- `examples/config.example.toml`：通用配置模板。
- `examples/config.toml`：保留的本地 NAS 配置。
- 不需要搜索的具体路径写在对应的 `[[index]]` 的 `exclude_paths` 中。

配置好后，下面的命令都不需要传配置文件。

## 日常使用

```bash
nasfind index update              # 更新所有索引，新索引自动初始化
nasfind index update research     # 只更新指定索引
nasfind soil moisture             # 搜索文件名，同时匹配两个关键词
nasfind -d research soil          # 只搜索指定索引
nasfind -i -l 20 ERA5             # 忽略大小写，最多返回 20 条
nasfind doctor                   # 检查配置和依赖
```

更新时在同一行原地刷新进度条、数字、速度和耗时，不逐次新增行；首次扫描用 spinner 表示活动。非终端输出不打印中间进度，只保留开始、完成等日志。预计剩余时间参考旧数据库的条目数，仅为估算；首次建立索引没有已知总量，不显示虚假的完成时间。终端中重要状态带颜色（青色：进行中，绿色：完成，黄色：提示，红色：错误）；重定向输出、设置 `NO_COLOR` 或 `TERM=dumb` 时不着色。加上 `--no-progress` 可关闭进度显示。

## 目录统计

目录统计统一使用 `nasfind stats`，不再提供 `dircount.py` 或 `dircount.sh`。旧版安装遗留的 `dircount` 命令可手动删除。

```bash
nasfind stats -n10                    # 查看条目最多的前 10 个目录
nasfind stats -d research -n20         # 只统计指定索引
nasfind stats /volume1/research/project  # 只统计某个子目录
nasfind stats --recursive false       # 只统计直接子项
```

统计**只查询数据库，不遍历索引目录**。条目包含文件和目录，默认递归统计（`--recursive true`）；`--recursive false` 只统计直接子项，多索引重复路径会去重。

`index update` 自动维护 `stats.db`，同时保存两种计数，让日常统计直接查询缓存。缓存位于首个索引数据库的同目录；首次使用或源数据库、配置变化时会自动重建。文件变化后，先更新索引再统计。

## 更多说明

- 所有命令的详细参数：`nasfind --help`、`nasfind index --help`、`nasfind stats --help`。
- [群晖安装](docs/synology.md) · [性能测试](docs/benchmark.md)
- 开发检查：`make check`；端到端测试：`make e2e`。Rust 单元测试位于 `tests/unit/`。

## 许可证

MIT。plocate 为独立依赖，遵循其自身许可证。
