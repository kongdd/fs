# nasfind

NAS 文件名搜索工具，默认由 **updatedb 建库、plocate 检索候选、Rust 实现 Everything 式语法与筛选**。支持多个索引、目录排除和增量更新；`nasfind stats` 可快速查看哪些目录包含最多的索引条目。实验性 Rust 建库代码已移至 `backup/rust-engine/`，不参与当前编译。

## 安装与配置（只需一次）

下载并解压 [Releases](https://github.com/kongdd/nasfind/releases) 中的 Linux 发布包。默认后端需 plocate/updatedb；可运行 `python3 setup-tools.py` 准备私有工具，检索本身不需要 Python。群晖用户参考[安装说明](docs/synology.md)。源码新增功能需使用新版构建。

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

增量更新由 updatedb 负责；进度在同一行刷新，不新增行。非终端输出只保留状态日志。重要状态带颜色，重定向输出、`NO_COLOR` 或 `TERM=dumb` 时不着色。`--no-progress` 关闭进度显示。

**旧 Rust 索引**：SQLite/Rust 格式已暂停支持。请将 `database` 配置改为新路径，再运行 `index update` 建立 plocate DB；不会自动覆盖原索引。

## 检索筛选

```bash
nasfind --ext nc,tif soil             # 扩展名筛选，可重复 --ext；忽略扩展名大小写
nasfind --path /volume1/research soil # 只查指定目录及其子目录
nasfind -r 'soil_.*[.]nc$'            # plocate POSIX 扩展正则
nasfind --offset 20 -l 20 soil        # 跳过筛选后的前 20 条，再返回 20 条
nasfind -b soil                      # 只匹配文件名，不匹配目录名
nasfind --json soil                  # JSON 输出；-0 输出原始 NUL 分隔路径
```

默认忽略 ASCII 大小写、匹配文件名；空格 AND，`|` OR，`!` NOT，`<...>` 分组，例如 `nasfind '<soil | rain> ext:nc;tif !backup'`。`-p` 匹配完整路径，`--case-sensitive` 区分大小写，`--locate` 恢复旧语义。支持范围和实测见 [Everything 式检索](docs/everything-search.md)。扩展名和路径筛选只读取索引，不访问 NAS 文件元数据；`--path` 按路径组件匹配，不解释通配符，相对路径以当前目录为基准，不解析符号链接。只有 `-e` / `--existing` 会检查结果是否仍存在。`--ext` 不覆盖配置中的排除规则；分页沿用数据库返回顺序，并非稳定排序。

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
- [Everything 式检索与实测](docs/everything-search.md)
- [群晖安装](docs/synology.md) · [性能测试](docs/benchmark.md)
- 开发检查：`make check`；端到端测试：`make e2e`。Rust 单元测试位于 `tests/unit/`，端到端测试位于 `tests/integration/`；`scripts/` 只放工具与基准脚本。

## 许可证

MIT。plocate 为独立程序，遵循其自身许可证。
