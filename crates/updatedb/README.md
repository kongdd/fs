# fs-updatedb

扫描目录、构建原生 schema v2 索引并增量更新；Unix 可选 plocate 后端。共享格式在 `../core`，查询实现位于 `../locate`。

```sh
cargo build --release --bin fs
fs updatedb
fs updatedb --folder /data/project
```

设计及历史性能记录见 [docs/updatedb](../../docs/updatedb/)。
