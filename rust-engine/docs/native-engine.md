# Rust 原生文件名检索引擎

## 使用与兼容

```bash
nasfind index update --engine rust   # 显式选择实验性 Rust 引擎
nasfind --ext nc,tif --path /volume1/research soil
nasfind -i -b -l 50 soil
nasfind -r 'soil_.*[.]nc$'
nasfind stats -n10
```

选择 Rust 原生库时，建库与检索不调用 plocate、updatedb、sort 或 Python；默认后端现为 updatedb/plocate。SQLite 随 Rust 程序内置，仅负责持久化、事务和页面查找；目录扫描、trigram 分词、压缩 posting、候选集交集和精确匹配由 Rust 实现，不依赖 SQLite FTS。

原生数据库与旧 plocate 数据库格式不同：

- **迁移时在配置中使用新的 database 路径**，运行 `index update --engine rust`；旧 DB 不会被自动覆盖。
- 查询按数据库头自动识别引擎，不需要指定 `--engine`。
- 继续更新旧数据库：`nasfind index update --engine plocate`。
- 一次查询不能混用两种格式；用 `-d` 选择同一格式的索引。
- DB 必须放在索引根目录之外。根目录变化需新建 DB。

## 索引结构

- `directories`：目录路径、设备号、inode、mtime/ctime（纳秒精度）。
- `entries`：条目 ID、目录 ID、原始字节 basename 和是否目录。公共目录前缀只保存一次。
- `postings`：ASCII 折叠的三字节 gram → 按目录分块的递增 ID 列表，使用差分 varint 编码。
- 公共目录前缀的 gram 使用“全部直接条目”标记，避免重复保存相同 ID 列表。
- `frequencies`：gram 的候选数，优先加载最稀有的 posting。
- 有效候选集按需交集，收益较低时提前停止；最终始终精确匹配，不能仅靠 gram 判断命中。
- 结果分批取出，支持在达到 `--limit` 后停止，不提前加载所有路径。

持久化路径为 BLOB，原始文本与 NUL 输出保留非 UTF-8 字节。JSON 使用 Unicode 替换字符表示非法字节，与之前的输出约定一致。

内存主要随**最大单目录条目数、访问到的目录前缀及候选 ID 数**增长，不把整棵树的文件路径同时载入内存。SQLite 页缓存上限在建库时设为约 32 MiB，但这不是总 RSS 的硬上限。

## 更新与一致性

逐个检查所有已知目录的 mtime、ctime 和 inode；未变目录复用其直接子项列表，不重新 `read_dir`。仍需检查其子目录，不能根据父目录日期跳过整个子树。

新增、删除、重命名会替换相关目录的条目和 posting，并清理消失的子树。目录排除规则变化触发重扫。`--folder` 仅更新所选子树，不重建全库；排除规则变化时须先完整更新。

- 不跟随目录中的符号链接；符号链接本身作为条目保存。
- 不可读取或扫描时发生变化的目录使更新失败，不静默丢弃旧记录。
- 已有库在单个 SQLite 事务中提交，错误回滚；首次建库使用临时文件，成功后原子 rename。
- 写入使用独立数据库锁；查询持有一致的只读事务快照。
- 无变更更新不改写 DB，统计缓存也无需重建。
- 正确性依赖文件系统提供可信的目录修改时间；不是实时监听引擎。

## 匹配语义与边界

CLI 默认使用 [Everything 式表达式](everything-search.md)。以下是 `--locate` 兼容模式与底层原生 matcher 的语义：

- 普通模式是子串匹配，多个模式 AND；含 `* ? []` 时按整个目标进行 glob 匹配。
- `-b` 仅匹配 basename；默认匹配完整路径。
- `-i` 为 ASCII 不区分大小写，不做 Unicode 归一化/模糊折叠。
- 原生正则采用 Rust regex 字节模式；旧后端使用 plocate POSIX 扩展正则，语法并不完全兼容。
- 少于三个字节的关键词、无可证明必需字面的正则等会扫描索引条目，但**不扫描 NAS 目录**。
- 扩展名/路径/配置排除在精确匹配后应用，再执行 offset、limit。
- 返回顺序为索引记录顺序，不承诺字典序或跨更新稳定分页。多索引可能重复返回相同路径；`stats` 会去重。
- 文件大小、日期筛选、排名与实时变更日志尚未实现。

## 正确性和性能测试

```bash
cargo test --all-targets
python3 tests/integration/test-native.py target/release/nasfind
bash tests/integration/test-e2e.sh target/release/nasfind   # 旧后端回归，需 plocate
python3 scripts/benchmark-native.py --files 100000 --runs 21 \
  --output native-benchmark.json
```

基准自动创建并清理独立目录和两套 DB，不使用用户配置、不更新现有索引。原生-only：加 `--native-only`。先建立两套索引，再交替测量查询；每个完整结果集和生成数据的字节级参考结果对比，限量查询检查数量和子集关系。GNU time 是可选的 RSS 测量依赖，不是检索引擎的运行依赖。

完整原始实测数据见 [10 万文件报告](native-benchmark-100k.json)。测试说明及结果汇总见 [性能实测](native-performance.md)。
