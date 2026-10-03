# nasfind v0.2.0

统一使用 updatedb 建库、plocate 查询候选，Rust 提供 Everything 式检索与目录统计。

- 默认忽略 ASCII 大小写、匹配文件名；支持 AND/OR/NOT、分组、通配符、ext:/path:/regex:。
- 支持扩展名/目录筛选、JSON/NUL 输出和过滤去重后的分页；`--locate` 兼容旧语义。
- 实验性 Rust 建库/索引后端及测试已移至 `backup/rust-engine/`，不参与主构建、CI 或打包。
- 旧 SQLite/Rust 索引明确报错，不静默覆盖；用新数据库路径重新建库。
- `nasfind stats` 支持 Top N、指定索引、子目录及递归/非递归计数，SQLite 仅用于统计缓存。
- `index update` 自动初始化新索引并维护统计缓存；进度单行刷新，重要日志支持终端颜色。
- 测试集中于 `tests/unit/` 和 `tests/integration/`，性能基准位于 `scripts/`。

Linux 发布包提供静态 Rust 程序；运行仍需独立的 plocate/updatedb 工具，可用 `setup-tools.py` 准备私有工具。检索本身不依赖 Python。详见群晖安装说明与 [Everything 检索说明](everything-search.md)。
