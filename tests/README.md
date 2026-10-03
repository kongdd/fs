# 测试

所有测试集中在本目录；各 crate 通过 `#[path]` 加载对应单元测试。

```sh
cargo test --locked --workspace --all-targets
cargo build --locked --bin fs
python3 tests/integration/test-native.py target/debug/fs
python3 tests/integration/test-everything.py target/debug/fs
```

Windows 可执行文件为 `fs.exe`。Everything 测试默认使用原生引擎；PATH 中有 plocate/updatedb 时额外验证同一组表达式。

Linux plocate 专属回归：

```sh
python3 tests/integration/test-ignore.py target/debug/fs
bash tests/integration/test-e2e.sh target/debug/fs
```

- `unit/core/`：配置、路径、匹配和终端输出。
- `unit/updatedb/`：扫描、编码、增量更新和 plocate 建库。
- `unit/locate/`：CLI、表达式、过滤、忽略和统计。
- `portable.rs`：跨平台索引/查询、离线根目录、Windows 路径。
- `ui/`：保留的 UI 测试，暂未在 CI 启用。
