# nasfind

**跨平台原生版 v0.3.0**：[直接下载](https://github.com/kongdd/nasfind/releases/latest)，支持 Linux、macOS、Windows，无需编译或 plocate；[使用说明](rust-engine/INSTALL.md)。下文介绍保留的 plocate 源码版，原生版功能以其说明为准。

NAS 文件名搜索工具，默认由 **updatedb 建库、plocate 检索候选、Rust 实现 Everything 式语法与筛选**。支持多个索引、目录排除和增量更新；`nasfind stats` 可快速查看哪些目录包含最多的索引条目。实验性 Rust 建库独立保留在 `rust-engine/`，不参与主程序编译；[块级索引优化](rust-engine/docs/chunk-index.md)包含设计与实测。

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
- `examples/config_nas.toml`：保留的本地 NAS 配置。
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

## 搜索用法

`nasfind search ...` 可简写为 `nasfind ...`，两者等价。默认匹配**文件名**、忽略 ASCII 大小写，查询索引而非实时扫描目录；新文件需先运行 `nasfind index update`。下面的索引名按实际配置替换，可用 `nasfind doctor` 查看。

### 基础搜索与索引选择

```bash
nasfind search soil                         # 文件名包含 soil
nasfind soil moisture                       # 同时包含 soil 和 moisture（AND）
nasfind search -d researches rain           # 只搜索 researches 索引
nasfind search -d github -d github_ssd README # 搜索多个索引
nasfind search --case-sensitive ERA5        # 区分大小写
nasfind search -p CMIP6                     # 匹配完整路径，包括目录名
nasfind search -b soil                      # 明确只匹配文件名（默认行为）
nasfind --config /path/to/config.toml search soil # 使用另一份配置
nasfind search -- --version                 # 搜索以 - 开头的文本
nasfind search --help                       # 查看全部搜索选项
```

### 表达式、短语与通配符

```bash
nasfind search 'soil | rain'                 # 包含任一关键词（OR）
nasfind search 'soil !backup'                # 包含 soil，但不包含 backup（NOT）
nasfind search '<soil | rain> ext:nc;tif !backup' # 分组、扩展名、排除组合
nasfind search '"my report"'                 # 搜索连续短语 my report
nasfind search 'ERA5*.nc'                    # 文件名 glob，匹配整个文件名
nasfind search 'soil_202?.nc'                # ? 匹配一个字符
nasfind search 'path:"my data" *.nc'         # 完整路径含 my data，文件名匹配 *.nc
```

空格是 AND，`|` 是 OR，`!` 是 NOT，`<...>` 用于分组，优先级为 NOT > AND > OR。带操作符、通配符或 `;` 的表达式请用单引号包住，避免 shell 提前解释；短语需保留表达式内部的双引号。默认 `!backup` 只检查文件名；要排除路径中的 backup，可用 `!path:backup`，或使用下面的 `ignore` 命令。

### 扩展名、目录范围与类型筛选

```bash
nasfind search --ext nc,tif soil             # 扩展名列表，忽略扩展名大小写
nasfind search --ext nc --ext tif soil       # 重复 --ext，与上面等价
nasfind search --path /volume1/Researches soil # 限制目录及其子目录
nasfind search --path . '*.nc'              # 限制当前目录及其子目录
nasfind search --dirs soil                  # 只显示推定目录
nasfind search --files soil                 # 只显示推定文件
nasfind search -d cmip6 --files --ext nc -l 20 ERA5 # 组合索引、类型、扩展名及条数
nasfind search -e soil                      # 检查结果是否仍存在（访问 NAS）
```

- `--path` 按路径组件限制目录子树，不解释通配符；相对路径以当前目录为基准，不解析符号链接。`path:文本` 则是完整路径文本匹配，不等同于 `--path`。
- `--dirs` 和 `--files` 互斥，不传则返回全部。只按最后一段文件名的**非空后缀**推断：`README`、`.gitignore`、`name.` 视为目录，`a.nc`、`.config.json` 视为文件。无后缀文件和带后缀目录可能被误分类，这是有意采用的快速近似。
- 扩展名、目录范围和类型筛选只读取索引，不访问 NAS 元数据；只有 `-e` / `--existing` 会检查存在性。`--ext` 不覆盖配置排除规则或查询忽略规则。

### 正则与旧 locate 模式

```bash
nasfind search -r 'soil_.*[.]nc$'            # plocate POSIX 扩展正则，默认匹配文件名
nasfind search -r -p '/ERA5/.*[.]nc$'        # 正则匹配完整路径
nasfind search -r --case-sensitive 'ERA5.*'  # 正则区分大小写
nasfind search --locate soil moisture       # 旧语义：完整路径、区分大小写、多模式 AND
nasfind search --locate -i -b soil          # 旧模式下忽略大小写，仅匹配文件名
```

`--locate` 不解析 Everything 表达式，`|`、`!` 等不会作为布尔操作符。完整语法范围与性能说明见 [Everything 式检索](docs/everything-search.md)。

### 挂载路径映射

```bash
nasfind search --mnt soil                   # 将结果转换为本地挂载路径
nasfind --mnt --files --ext nc ERA5         # 映射 + 文件/扩展名筛选
nasfind search --mnt --path /volume1/Researches rain # 范围仍用 NAS 原路径
```

| NAS 路径前缀 | `--mnt` 输出前缀 |
|---|---|
| `/volume1/CMIP6` | `/mnt/z` |
| `/volume2/GitHub` | `/mnt/x` |
| `/volume1/Researches` | `/mnt/y` |
| `/volume1/CUG-hydro` | `/mnt/o` |

仅映射输出路径，按完整路径组件匹配，其余路径不变。文本、JSON 和 NUL 输出都支持。搜索表达式、`--path`、忽略规则及 `--existing` 仍使用 NAS 原路径，不检查本地挂载是否存在。

### 分页、JSON 与管道输出

```bash
nasfind search -l 20 soil                   # 筛选后的前 20 条
nasfind search --offset 20 -l 20 soil       # 跳过 20 条，再返回 20 条
nasfind search --json soil                  # JSON 数组：[{"path":"..."}, ...]
nasfind search --mnt --json soil > results.json # 保存映射后的 JSON
nasfind search -0 soil > results.paths0     # 原始 NUL 分隔路径，保留非 UTF-8 文件名
nasfind search -0 soil | while IFS= read -r -d '' path; do
    printf '%s\n' "$path"                   # Bash：逐项读取，正确处理空格与换行
done
```

先执行表达式、忽略规则及筛选，再分页；OR 分支的重复路径会先去重。结果沿用分支和数据库返回顺序，并非稳定排序。`--json` 与 `-0` 互斥，空结果 JSON 为 `[]`；JSON 中非 UTF-8 文件名字节会显示为替换字符，需保留原始字节时用 `-0`。文本输出每行一个路径，文件名可能含换行，脚本处理优先使用 NUL 输出。

## 查询时忽略目录

```bash
nasfind ignore add node_modules cache FY4B # 一次添加多个目录名，隐藏同名目录及其子项
nasfind ignore list             # 列出忽略的目录名
nasfind ignore rm node_modules cache FY4B  # 一次取消多个
```

立即生效，无需重建索引，适用于普通、正则和 `--locate` 查询；先过滤再分页。参数是**目录名而非路径**（例如 `node_modules`、`cache with spaces`），不接受 `/data/cache`、`./cache`。按完整路径组件精确匹配，区分大小写，不解释通配符：`cache` 不会匹配 `cache-other`。仅按名字过滤，不访问文件系统，因此同名文件也会被隐藏。`add` 和 `rm` 均支持一次传多个目录名，带空格的名字需加引号；整批先校验，任一参数无效则全部不写入，重复添加不会产生重复规则。规则仍保存在当前配置文件旁的 `<config>.ignore.json`，可用 `--config` 切换；不修改建库规则或目录统计。旧版绝对路径规则需手动改为目录名（不会自动扩大忽略范围）。取消忽略只能恢复数据库已有的条目。

## 图形界面

Everything 风格的 TypeScript 界面位于 [UI/](UI/README.md)：

```bash
cd UI
npm ci
npm run dev
```

打开 `http://localhost:5173`。默认是明确标注的本地演示数据；HTTP 查询接口已预留，但当前尚无服务端，不能直接连接 NAS 索引。

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

- 所有命令的详细参数：`nasfind --help`、`nasfind search --help`、`nasfind ignore --help`、`nasfind index --help`、`nasfind stats --help`。
- [Everything 式检索与实测](docs/everything-search.md)
- [群晖安装](docs/synology.md) · [性能测试](docs/benchmark.md)
- 开发检查：`make check`；端到端测试：`make e2e`。Rust 单元测试位于 `tests/unit/`，端到端测试位于 `tests/integration/`；`scripts/` 只放工具与基准脚本。

## 许可证

MIT。plocate 为独立程序，遵循其自身许可证。
