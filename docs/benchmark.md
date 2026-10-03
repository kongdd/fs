# 群晖 NAS 实测

需要 Python 3.8+。测试应在 NAS 的 SSH 终端运行，DB 放在计划实际使用的位置。
已有 [Everything 式检索的 10 万文件实测](everything-search.md)，但不能代替真实目录、设备负载与部署方式的测试。

## 1. 准备与日常操作

```bash
uname -m
./nasfind --version
./nasfind --config config.toml doctor
./nasfind --config config.toml index research --no-progress
./nasfind --config config.toml -i -l 50 soil
./nasfind --config config.toml -d research '*.nc'
./nasfind --config config.toml stats -d research -n10
./nasfind --config config.toml index --folder /volume1/research/project --no-progress
```

确认路径、大小写、指定 DB、无匹配、含空格文件名、通配符操作符合预期。
通配符需要引号；多个关键词是 AND；`-e` 会访问 NAS 检查文件存在，普通搜索不需要它。
日常可设置 `NASFIND_CONFIG=/绝对路径/config.toml`，省去每次指定 `--config`。

## 2. 查询耗时

解压安装包后，运行目录内的脚本；源码仓库中脚本位于 `scripts/benchmark.py`。
选至少三个真实查询：常用词、匹配很多的词、没有匹配的词。

```bash
python3 benchmark.py --nasfind ./nasfind --config config.toml \
  --index research --query soil --query '.nc' --query nasfind_no_match_987654 \
  --runs 20 --limit 50 --label 'DSM 7.4; NAS 本机; DB 在 SSD' \
  --output query-results.json
```

报告记录首次查询、后续查询 P50/P95 和返回数量。默认最多 50 个结果，贴近日常使用。
再用 `--limit 0` 测量全部匹配，观察大量结果输出时的耗时。
脚本不清理内核缓存，因此首次查询并不保证冷缓存。计时包含进程启动、配置解析、
查询、过滤与写入临时文件，文件计数在计时结束后完成；不包含终端渲染。
后续统计不包含首次查询，不足 20 次时 P95 的参考价值有限。

## 3. 建库和局部更新耗时

下面命令会更新真实 DB，但不会创建、删除或修改 NAS 上的个人文件：

```bash
python3 benchmark.py --nasfind ./nasfind --config config.toml \
  --index research --update --folder /volume1/research/project \
  --query soil --runs 20 --timeout 7200 --output update-results.json
```

先记录连续两次普通更新，然后测量局部更新。原生引擎只修改子树相关条目；旧 plocate 后端的局部合并包括重建整个主 DB。
当前 `index` 命令还会刷新统计缓存，因此报告中的更新耗时包含扫描、写库和统计缓存维护，不能视为纯扫描耗时。
若 DB 已存在，第一次普通更新不是首次建库；第二次也不能保证期间没有文件变化。
若要测首次建库，请另配一个输出 DB 路径，不要删除正在使用的 DB。
原生引擎的局部更新保留目录复用信息。仅旧 plocate 后端的局部合并会丢失该信息，下一次普通更新耗时可能上升，值得另测一次。

在自己建立的测试子目录中添加、删除和重命名少量文件，运行 `index --folder`，
确认记录正确变化；其他目录中的新文件应保持未索引状态，直到更新该目录或全量更新。

比较实际部署组合：NAS 本机 DB 在 HDD、NAS 本机 DB 在 SSD；若客户端也运行 Linux，
再比较 NAS 建库后复制 DB 到客户端 SSD 查询。各轮使用相同查询、结果上限和过滤规则，
分别记录执行位置，避免同时进行其他大规模 NAS 任务。

报告能比较不同更新和检索场景的总耗时；扫描、写库与统计缓存维护需另外分段计时才能精确归因。

## 4. Everything 模式与直接 plocate 对照

```bash
python3 scripts/benchmark-everything.py --nasfind target/release/nasfind \
  --files 100000 --runs 7 --output everything-benchmark.json
```

发布包中使用 `--nasfind ./nasfind`。脚本在 `~/.cache` 创建并清理独立的合成文件目录和数据库，不更新现有索引。报告保存完整样本，并对查询结果进行参考集合校验；详见 [范围和实测说明](everything-search.md)。

实验性 Rust 引擎、测试与性能报告保存在 `rust-engine/`，独立构建、不参与主程序打包；最新前后对照见[块级索引](../rust-engine/docs/chunk-index.md)。
