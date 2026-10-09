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

## 用法

### 建库与更新

```sh
fs updatedb                          # 更新全部索引；缺失数据库自动初始化
fs updatedb research                 # 只更新指定索引
fs updatedb research github          # 更新多个索引
fs updatedb init                     # 只初始化缺失数据库，不改动已有库
fs updatedb init research
fs updatedb -j 8                     # Rust 引擎使用 8 个扫描线程
fs updatedb --folder /data/project   # 只更新子目录，合并到所属索引
fs updatedb --folder /data/a --folder /data/b
fs updatedb --no-progress            # 关闭动态进度
```

`--folder` 与索引名不能同时使用；子目录须属于已配置的索引。默认单线程扫描；更新后才会反映文件的新增、删除和改名。`index` 是 `updatedb` 的兼容别名。

### 默认搜索与大小写

```sh
fs soil                             # 等同于 fs locate soil
fs locate soil                      # 文件名包含 soil，忽略 ASCII 大小写
fs locate -C Soil                   # 区分大小写；-C 等同于 --case-sensitive
fs locate -- '-notes'                # 搜索以连字符开头的词，避免当作选项
```

默认匹配 **basename（文件名）**，输出 **完整路径**。普通文本按子串匹配，不要求占满整个文件名；查询词含 `/` 时自动匹配完整路径。未指定索引时查询配置中的全部索引。`search` 是 `locate` 的兼容别名。

### 表达式、空格与引号

空格表示 AND，`|` 表示 OR，`!` 表示 NOT，`<...>` 表示分组；优先级为 NOT > AND > OR。

```sh
fs locate 'soil rain'                # 文件名同时包含 soil 和 rain
fs locate 'soil | rain'              # 包含任一关键词
fs locate 'soil !backup'             # 包含 soil，但不包含 backup
fs locate '<soil | rain> exts:nc !backup'
fs locate 'Kong 2024 *.pdf'           # 三个条件 AND，不要求 Kong 与 2024 相邻
fs locate '"Kong 2024" *.pdf'         # 必须包含连续文本 Kong 2024
fs locate '"a ! b" *.txt'            # 引号内的空格、!、| 等作为普通文本
```

示例使用 POSIX shell 的单引号保护整个表达式，防止 shell 提前展开通配符或解释 `|`、`!`、`<`、`>`；内层双引号保留带空格的查询词。双引号只改变表达式分词，词内的通配符仍会生效。Windows 的引号规则依所用 shell 而定。

### 通配符与扩展名

```sh
fs locate '*.pdf'                    # 文件名以 .pdf 结尾，默认也匹配 .PDF
fs locate 'Kong*.pdf'                 # 以 Kong 开头，以 .pdf 结尾
fs locate '*2024*'                    # 包含 2024
fs locate 'report_?.pdf'              # ? 匹配一个字符（原生引擎按字节匹配）
fs locate 'report_[0-9].pdf'          # 字符集合或范围

fs locate 'exts:pdf'                  # 单个扩展名
fs locate 'exts:docx,pdf'             # 多个扩展名，逗号分隔，任一匹配
fs locate 'Kong exts:docx,pdf'        # 关键词与扩展名组合
fs locate --exts docx,pdf Kong        # 命令行过滤写法
fs locate --exts docx --exts pdf Kong # 也可重复选项
```

通配符匹配整个文件名；例如 `*.pdf` 匹配 `report.pdf`，不匹配 `report.pdf.bak`。扩展名过滤始终忽略 ASCII 大小写，可带前导点，如 `exts:.pdf`。旧写法 `ext:`、`--ext` 及表达式中的分号分隔仍兼容，建议使用 `exts:` 与逗号。

### 路径、索引和类型过滤

```sh
fs locate -p Kong                    # --include-path：将目录路径也纳入匹配
fs locate 'path:papers Kong'         # 路径包含 papers，文件名包含 Kong
fs locate 'papers/Kong'              # 含 / 的查询词自动匹配完整路径
fs locate --path '/data/my papers' Kong # 限定目录子树，不检查目录是否在线

fs locate -d research Kong          # -d / --index：只查询一个索引
fs locate -d research -d github Kong
fs locate --files 'exts:pdf'         # 只返回按后缀推断的文件
fs locate --dirs project            # 只返回按后缀推断的目录
fs locate -e Kong                   # --existing：只返回当前仍存在的路径
```

`path:` 是路径文本匹配，`--path` 是按路径组件边界限定子树，二者不同。`--path` 可用绝对路径或相对当前目录的路径；不会把 `/data/a-other` 当作 `/data/a` 的子目录。

`--files` 与 `--dirs` 互斥，且**不是读取元数据后的真实类型判断**：有非空扩展名视为文件，否则视为目录。因此 `README`、`.gitignore` 会被视为目录，`folder.v1` 会被视为文件；`file:`、`folder:` 表达式暂不支持。默认匹配文件名，无需额外选项；匹配完整路径时使用 `-p`。

查询默认只读取索引，支持源目录离线；`-e` 才会逐项检查结果是否存在，可能访问磁盘或 NAS。

### 正则匹配

```sh
fs locate -r '^Kong.*[.]pdf$'        # --regex：整条查询按正则处理，默认匹配文件名
fs locate -r -C '^Kong.*[.]pdf$'     # 区分大小写
fs locate -r -p '/papers/.*[.]pdf$'  # 正则匹配完整路径
fs locate 'regex:"^Kong.*[.]pdf$" !backup' # 正则词与其他表达式组合
```

Rust 索引使用 Rust 正则语法。`--regex` 不解析 Everything 表达式；`regex:` 则只将该词作为正则。各模式均默认忽略 ASCII 大小写；匹配完整路径统一使用 `-p`。

### 分页、JSON 与管道

```sh
fs locate -n20 Kong                 # -n / --nlimit：最多 20 条，必须大于 0
fs locate -o20 -n20 Kong            # -o / --offset：跳过前 20 条后取 20 条
fs locate --offset 20 --nlimit 20 Kong
fs locate --json 'exts:docx,pdf'     # JSON 数组；每项为 {"path":"完整路径"}
fs locate -0 '*.pdf' | xargs -0 -n1 printf '%s\n'
```

默认每行输出一个完整路径，顺序由查询后端决定，不保证按名称排序。分页在过滤后应用。`-0`（`--null`）使用 NUL 分隔，适合含空格、制表符或换行的文件名；与 `xargs -0` 配合可避免路径被拆分。`--json` 与 `--null` 互斥；没有结果时 JSON 输出 `[]`。

### 查询时隐藏目录

```sh
fs ignore add cache node_modules 'cache with spaces'
fs ignore list
fs ignore rm cache node_modules 'cache with spaces'
```

规则是**目录名**，不是完整路径：精确、区分大小写地匹配任意深度的同名路径组件，并隐藏其后代。重复添加不会重复保存。规则写入查询配置旁的 `<配置文件>.ignore.json`，不修改 TOML、不重建索引；删除规则也不会恢复建库时已排除的记录。

### 统计、诊断与帮助

```sh
fs stats                            # 默认列出前 10 个目录，统计全部后代
fs stats -n20                       # -n / --top：排行数量
fs stats /data/project -n20          # 限定子树；也可用相对路径
fs stats -d research -d github
fs stats --recursive false           # 只统计各目录的直接子项
fs doctor                           # 检查配置、根目录、数据库及后端依赖
fs --help
fs locate --help
fs updatedb --help
fs stats --help
```

统计基于索引，不遍历源文件系统；计数包含文件和目录，不等同于文件数量。默认递归统计中，父目录与子目录的计数会重叠。

## 配置

### 最小配置与建库排除

配置文件使用 TOML；根目录和数据库路径应为绝对路径。可用 `fs init config.toml` 生成示例，`--force` 允许覆盖已有配置。

```toml
engine = "rust"

[filters]
exclude_dirs = [".git", "node_modules"]
exclude_extensions = ["tmp", "pyc"]
exclude_files = [".DS_Store"]

[[index]]
name = "research"
root = "/data/research"
database = "/data/fs-db/research.db"
exclude_paths = ["cache", "private/archive"]
```

全局 `[filters]` 应用于各索引；`[[index]]` 内同名字段覆盖全局字段。相对的 `exclude_paths` 以该索引 `root` 为基准。`exclude_dirs` 用于建库时跳过目录，与可即时切换的 `fs ignore` 不同。修改建库排除规则后，应更新整个索引，而不是只更新某个子目录。

```sh
fs -c config.toml updatedb           # -c / --config：指定本次使用的配置
fs -c config.toml locate soil
export FS_CONFIG=/absolute/path/config.toml
```

路径统一使用 `/`；Windows 可用 `C:/Users` 和 `//server/share`。多索引、`outdir`、`update_database` 与 `search_database` 等配置规则见 [使用参考](docs/usage.md)。

### 切换建库引擎

```sh
fs updatedb --engine rust -j 8 # 临时指定本次建库引擎
fs config engine rust      # 保存默认引擎
fs config engine plocate   # 仅 Linux，需安装 plocate/updatedb
fs config engine           # 查看当前默认引擎
export FS_ENGINE=rust      # 仅对当前进程环境生效
```

建库引擎优先级为 `--engine` > `FS_ENGINE` > 配置文件 `engine` > 默认 `rust`。查询自动识别数据库格式；切换建库引擎不会转换已有数据库，也不能用不同引擎更新同一个库。

### 记住查询与建库配置

路径保存为绝对路径。之后 `locate`、`stats`、`ignore` 使用 locate 配置，`updatedb` 使用 updatedb 配置。`-c` 与 `FS_CONFIG` 仍优先。

```sh
fs config set locate search.toml
fs config set updatedb updatedb.toml
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
fs -c config/seach.toml locate soil
# 或先保存配置，以后可省略 -c
fs config set locate config/seach.toml
fs config set updatedb config/updatedb_nas.toml
```

使用前须修改 [config/](config/) 中的用户名、扫描目录和数据库路径，并挂载 NAS。Rust 索引的查询配置 `root` 应设为本机挂载路径，`database` 应设为本机可读取的数据库路径。结果、路径过滤及存在性检查均使用查询配置的 `root`，无需重建索引。例如 NAS 建库时为 `/volume1/Researches`，Mac 查询时可改为 `/mnt/y`。

## 仓库内容

- `crates/main.rs`：命令行入口。
- `crates/core/`：配置、路径匹配和共享索引格式。
- `crates/updatedb/`：扫描、建库和增量更新。
- `crates/locate/`：检索、过滤和统计。
- `config/`、`scripts/`、`tests/`：配置示例、辅助脚本和测试。

详细配置与限制见 [使用参考](docs/usage.md)，开发验证见 [测试说明](tests/README.md)，辅助工具见 [脚本说明](scripts/README.md)。
