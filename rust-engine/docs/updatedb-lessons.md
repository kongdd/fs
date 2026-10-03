# 学习 updatedb：首次建库优化（v1 历史记录）

当前 v2 已实现分块压缩与全局块级倒排，见 [块级索引](chunk-index.md)。以下数据与 schema 说明仅对应 v1。

## 阅读的源码

参考的是 **plocate 版 updatedb**，不是只看命令参数：

- [updatedb.cpp](https://git.sesse.net/?p=plocate;a=blob;f=updatedb.cpp;hb=HEAD)：`scan()`、`opendir_noatime()`、`get_dirtime_from_stat()`、旧记录复用。
- [database-builder.cpp](https://git.sesse.net/?p=plocate;a=blob;f=database-builder.cpp;hb=HEAD)：`EncodingCorpus`、`PostingListBuilder`、分块压缩及最终写出。

本次阅读快照 SHA-256：

- updatedb.cpp：`e4f0dbaf1310be9d746dcd9ac06aa0aed9a719bf4e7be787769b7d7fa3691bda`
- database-builder.cpp：`9dde8799f47260ae07fdaac1c2b1cfe0dc6eb57bc9fbafaf3bece07370bb281e`

上游是 GPL 代码；这里借鉴公开的算法/架构思路，独立用 Rust 实现，没有复制上游源码进 MIT 项目。

## 已采用

| updatedb 的思路 | Rust 中的实现 |
|---|---|
| 目录条目缓存 basename，而不是重复保存完整前缀 | 扫描缓存 `(OsString, is_dir)`，只有遍历或路径排除时拼完整路径 |
| 用 `dirent.d_type`，未知类型才 stat | 使用 Linux `DirEntry::file_type()` 的对应快路径，不逐文件读完整 metadata |
| 排除规则提前处理 | 目录名 HashSet、预先解析排除路径；被排除目录不进入扫描队列 |
| 单调 docid，通过最后一个 ID 去重 | 构建 posting 时检查最后一个 entry ID，不再对每个文件的 gram 分配、排序和去重 |
| 扫描/编码/最终写出分开 | 新增扫描、编码/写库、最终聚合与提交的分段计时 |
| 新库流式积累、最后构建查询结构 | 首次建库不反复 UPSERT gram 频率；最后一次有序 GROUP BY 生成频率表，再批量创建两个辅助索引 |
| 构建新库不处理旧库删除与复用 | 首建跳过旧目录查询、清理 posting、删除旧条目、失效子树检查和重复 stamp 更新 |
| 内存中快速累计 posting | HashMap 累计，最终唯一键排序写出；每个目录复用 prepared statements 和临时 suffix 缓冲区 |
| 完成后发布 DB | 保留临时库、事务、FULL synchronous 和原子 rename，不靠关闭持久化换速度 |

这里仍采用**entry ID**，不是上游的 filename-block ID。更新时继续维护频率增量，只有新库走延后索引与一次性聚合路径。磁盘 schema/version 不变。

## 暂未采用，以及原因

- **约 32 文件一块 + zstd 压缩 + block-level posting**：这是 plocate 体积小、写出快的重要原因，需要一起改候选展开、压缩读取及增量更新格式，不能只换编码器。是下一阶段优先方向。
- **密集 trigram 指针表**：上游为 2^24 个 gram 分配指针，在 64 位机器上仅表本身约 128 MiB。当前先保留稀疏结构，避免低内存 NAS 固定承担这笔开销。
- **openat/fstatat 与目录 FD 遍历**：能改善超长路径和 TOCTOU 防护；当前仍有完整路径访问，不能宣称已经解决 PATH_MAX 问题。独立目录 FD 实现还需处理资源上限、符号链接和竞态。
- **O_NOATIME + EPERM 回退**：可减少读取引起的 atime 写入，但与权限、挂载模式有关，不能假设每个 NAS 都受益。
- **最近约 3 秒内修改的目录不信任缓存**：上游用于防止时间戳精度/时钟源导致漏扫。当前比较 inode、mtime/ctime 纳秒值并检查扫描前后变化，但未实现该时间窗口保护；粗粒度时间戳文件系统仍需额外防护。

## 优化前后对照

不是把上次的一次计时与本次最好结果拼起来：保存优化前可执行文件，同一份 10 万文件数据，每次使用新 DB，交替运行两个版本。包含统计缓存维护，不清空 OS 缓存，每次全结果和选择性查询都校验参考结果。原始报告记录两个二进制的 SHA-256。

| 布局 | 重复次数 | 优化前中位数 | 优化后中位数 | 加速比 |
|---|---:|---:|---:|---:|
| 1000 目录 × 100 文件 | 每版 5 次 | 4.871 s | 3.079 s | 1.58×，耗时减少约 37% |
| 1 目录 × 100000 文件 | 每版 3 次 | 2.194 s | 1.561 s | 1.41×，耗时减少约 29% |

峰值 RSS 的权衡：

- 多目录：约 28.52 → 34.72 MiB，延后建索引的批量排序增加内存。
- 单个大目录：约 48.59 → 39.89 MiB，basename 缓存减少路径重复占用。

文件系统缓存和系统负载会影响结果；这些不是冷盘保证，也不能直接推断对 plocate 的胜负。优化后多目录构建的阶段中位数约为扫描 0.301s、编码/写库 1.705s、最终聚合/索引/提交 0.595s，瓶颈仍主要在索引编码与写出。

原始报告：

- [多目录 10 万文件](native-init-benchmark-100k.json)
- [单目录 10 万文件](native-init-benchmark-flat-100k.json)

复现方式：

```bash
# 修改代码前保存旧可执行文件；保存路径应在仓库和扫描目录之外。
cp target/release/nasfind /安全的临时目录/nasfind-before
cargo build --release
python3 scripts/benchmark-init.py --before /安全的临时目录/nasfind-before \
  --files 100000 --runs 5 --output init-benchmark.json
python3 scripts/benchmark-init.py --before /安全的临时目录/nasfind-before \
  --files 100000 --per-directory 100000 --runs 3 --output flat-benchmark.json
```

测试使用独立临时目录和新数据库，不读取用户配置，不改动已有 NAS 索引。
