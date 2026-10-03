# 测试目录

- `unit/`：Rust 单元测试，通过源模块的 `#[path]` 引入，以测试内部实现；源文件不包含测试函数。
- `integration/`：对可执行文件进行端到端验证，测试数据与数据库均在独立临时目录内，不改动现有 NAS 索引。
  - `test-everything.py`：Everything 式检索与真实 plocate 后端、退役索引保护。
  - `test-e2e.sh`：plocate/updatedb 后端及统计、配置、安装等回归。

从仓库根目录运行：

```bash
make check
make e2e
```

`make e2e` 需要 Python 3、plocate、updatedb、plocate-build 和 GNU sort。可先运行 `python3 scripts/setup-tools.py` 准备工具。

- `ui/`：TypeScript 查询、API 数据约定及界面交互测试（Vitest + jsdom），执行 `npm --prefix UI ci && npm --prefix UI test`。

性能基准脚本保留在 `scripts/benchmark*.py`，不属于回归测试。
