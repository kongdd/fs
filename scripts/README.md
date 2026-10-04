# 脚本

均从仓库根目录运行。基准脚本需要 Unix；发布打包需要 Python 3.11+。每个 Python 脚本均支持 `--help`。

## benchmarks/

- `native.py`：合成语料，原生与 plocate/旧二进制对照。
- `init.py`：初始化建库前后对照。
- `real.py`：真实语料副本测试，不修改源目录。
- `config.py`：按配置选取真实目录进行计时。
- `everything.py`：Everything 与 plocate 对照。
- `plocate.py`：已配置数据库的日常工作负载；`--update / --folder` 会更新数据库。
- `common.py`：共享子进程计时及 RSS 测量。

```sh
cargo build --release --bin fs
python3 scripts/benchmarks/native.py --fs target/release/fs --native-only
python3 scripts/benchmarks/plocate.py --config examples/config_nas.toml --query soil
```

## release/

`package.py TARGET` 仅打包已编译二进制，默认读取 `target/TARGET/release/fs`，支持 `CARGO_TARGET_DIR` 和 `--out`。

```sh
cargo build --release --bin fs --target aarch64-apple-darwin
python3 scripts/release/package.py aarch64-apple-darwin
```

## tools/

- `setup-plocate.py`：Linux 私有 plocate 工具，仅在显式选择 plocate 引擎时需要。
- `index-and-count.sh`：更新当前配置的索引，再显示目录排行；通过 `FS_CONFIG` 指定配置。
