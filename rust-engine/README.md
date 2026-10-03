# Rust 原生索引备份（暂停使用）

此目录保存移出主程序的实验性 Rust 建库/索引后端，未提交、未删除用户数据库。

- `src/native.rs`：SQLite 索引、扫描、增量更新、trigram posting 与原生查询实现。
- `src/main.rs`、`indexer.rs`、`search.rs`、`everything.rs`：停用前的接入代码快照，仅作恢复/对照参考。
- `tests/`：原生单元测试、端到端测试与双后端 Everything 测试快照。
- `scripts/`：自建库性能对照脚本。
- `docs/`：设计说明、性能报告、updatedb 学习记录及原始 JSON。
- Cargo 文件：当时的依赖快照；rusqlite 在主程序中仍用于统计缓存，不能因暂停原生索引而移除。

这不是独立可构建的 crate，不参与主程序编译、CI 或打包。若继续研发，应审阅这些快照并重新接入，不要直接用旧 main/search/indexer 覆盖后续代码。

主程序当前由 updatedb 建库、plocate 查询候选，Rust 保留 Everything 表达式和纯匹配逻辑（`src/matching.rs`）。既有 Rust/SQLite 索引被明确拒绝，不会静默覆盖；改用新 DB 路径后重新建库。
