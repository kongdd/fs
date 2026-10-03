# Everything 式检索：当前范围与性能

当前是核心语法子集，不是 Everything/ES 的完整复刻。默认由 updatedb 建库、plocate 提供候选，Rust 解析表达式、精确匹配、应用筛选、去重与分页。实验性 Rust 建库代码已备份到 `backup/rust-engine/`。主程序拒绝旧 SQLite/Rust 索引，需配置新 DB 路径再重新建库，不自动覆盖。

## 已支持

- 默认 ASCII 不区分大小写、默认匹配 basename；`--case-sensitive` 区分大小写，`-p/--match-path` 匹配完整路径。
- 空格 AND、`|` OR、`!` NOT、`<...>` 分组；优先级 NOT > AND > OR。
- 双引号保留短语和操作符；Linux shell 下需要将引号本身传入表达式。
- 子串、`* ? []` glob、`ext:nc;tif`、`path:文本`、`regex:表达式`。
- `--ext`、`--path`、`--offset`、`--limit`；OR 分支去重后才分页。
- JSON 和原始 NUL 分隔输出；非 UTF-8 文件名可通过 `-0` 保留。
- `--locate` 恢复旧语义（默认区分大小写、完整路径、多模式 AND）；原来的 `-i` 仍表示忽略大小写，不照搬 ES 的同名短选项。

```bash
nasfind soil moisture
nasfind '<soil | rain> ext:nc;tif !backup'
nasfind '"my report"'
nasfind 'path:"my data" *.nc'
nasfind --case-sensitive SOIL
nasfind -p research
nasfind --path /volume1/research --offset 20 -l 20 soil
```

`path:` 是完整路径文本匹配；`--path` 才是按路径组件限制目录子树。glob 匹配整个目标。表达式最多 256 token、64 个展开分支。仅否定、仅扩展名或无安全字面候选的正则可能扫描整个索引，而不是扫描 NAS。OR 输出按分支和后端返回顺序，不是全局排序。

## 未支持

- `file:`/`folder:` 类型过滤：plocate DB 不保存可靠类型字段，目前明确报错。
- 大小、创建/修改日期、属性过滤；排序、全词匹配、模糊匹配、完整 Unicode 折叠。
- 结果高亮、交互式界面、实时监听及 Everything 的完整函数集合。

查询默认不为筛选补读 NAS 文件元数据；`-e/--existing` 是显式的存在性检查。现有配置中的排除规则仍然有效。

## 10 万文件实测

1000 目录 × 100 文件，共用一份 plocate DB。每个命令一次预热，之后交替运行 7 次；以下为 CLI 时间中位数，包含启动和输出捕获。所有完整结果经过字节级参考集合校验。

| 查询 | Everything 模式 | 直接 plocate |
|---|---:|---:|
| 稀有词，1 条结果 | 13.87 ms | 14.53 ms |
| AND，25000 条 | 115.24 ms | 103.52 ms |
| OR，50000 条 | 126.59 ms | 157.44 ms |
| NOT，75000 条 | 170.46 ms | 无对应谓词 |
| 扩展名列表，50000 条 | 124.45 ms | 166.07 ms |
| 限量 50 条 | 7.81 ms | 8.25 ms |

plocate 没有 OR 开关，表中的 OR 和扩展名列表基线采用等价 POSIX 扩展正则，**不能据此宣称 Rust 比 plocate 的索引检索更快**。少量结果的差异受系统噪声影响；大结果 AND 当前约增加 11% 开销。

当前优化空间：AND 候选仅选择每个分支的最长必需字面串，未把全部正向条件同时下推；OR 需要多个子进程和去重；NOT/扩展名等通常在 Rust 后过滤。尚未测试千万级索引，也没有冷盘延迟保证。

原始数据：[everything-benchmark-100k.json](everything-benchmark-100k.json)。复现：

```bash
python3 scripts/benchmark-everything.py --files 100000 --runs 7 \
  --output everything-benchmark.json
```

测试只创建临时数据和新数据库，不读取用户配置、不改动现有 NAS 索引。
