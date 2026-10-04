# fs-updatedb

扫描目录、构建原生 schema v4 索引并增量更新（根目录保存一次，目录路径相对编码；不兼容或迁移旧版原生数据库，须重建）；Unix 可选 plocate 后端。建库入口为 `src/index_builder.rs`，共享存储与读取在 `../core/src/index_store.rs`，查询入口为 `../locate/src/index_search.rs`。

```sh
cargo build --release --bin fs
fs updatedb
fs updatedb --folder /data/project
```

设计及历史性能记录见 [docs/updatedb](../../docs/updatedb/)。
