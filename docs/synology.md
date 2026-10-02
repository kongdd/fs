# 群晖 DSM 7.x：免编译运行

目前没有在用户的 DSM 7.4 上做实机验证。Release 的 nasfind 为静态 musl 二进制，
CI 分别在 x86_64 和 aarch64 Linux 上测试，避免依赖 DSM 自带的 glibc 版本。
这不是套件中心的 SPK，也不包含 Mac/Windows 程序。

## 选择安装包

在群晖 SSH 终端执行 `uname -m`：

| 输出 | Release ZIP |
|---|---|
| `x86_64`（Intel/AMD） | `nasfind-0.2.0-linux-x86_64.zip` |
| `aarch64`（64 位 ARM） | `nasfind-0.2.0-linux-aarch64.zip` |

32 位 ARM 暂不支持。DSM 版本本身不能确定 CPU 架构。

DSM 7.4 也没有统一的 Linux 内核版本。官方
[7.4-90075 工具链目录](https://archive.synology.com/download/ToolChain/toolchain/7.4-90075)
列出不同平台使用的内核，例如 Avoton/Braswell 为 3.10.108，r1000/v1000 为
4.4.302，r1000nk/v1000nk、RTD1619B 为 5.10.55。工具链列表不代替实机检测；
在 NAS 上运行 `uname -rm`，即可得到实际内核版本和 CPU 架构。

## 解压后直接运行

需要 Python 3.8+（可通过群晖提供的 Python 套件安装）和网络连接。Python 仅用于
准备依赖与运行基准脚本，nasfind 本身不需要 Python。

```bash
unzip nasfind-0.2.0-linux-x86_64.zip
cd nasfind-0.2.0-linux-x86_64
./nasfind --version
python3 setup-tools.py
./nasfind init ./config.toml
```

`setup-tools.py` 从官方 Alpine 软件源下载 plocate、GNU coreutils 和所需运行库，
保存在本目录的 `tools/` 下，不安装到 DSM 系统目录，不需要 root 或编译器。
工具通过私有 musl 加载器运行；`PACKAGES.json` 记录来源与版本。
下载或工具自检失败时不会发布半成品工具目录。

编辑 `config.toml`，设置实际扫描目录和 DB 路径，例如：

```toml
[[index]]
name = "research"
root = "/volume1/research"
database = "/volume1/nasfind-db/research.db"
```

保留并按需调整生成配置中的 `[filters]`。局部某项未定义时继承全局，
定义后覆盖对应全局列表，`[]` 关闭该项过滤。
不要同时保留指向不存在目录的示例索引。

```bash
./nasfind --config ./config.toml doctor
./nasfind --config ./config.toml index research --no-progress
./nasfind --config ./config.toml -i -l 50 soil
./nasfind --config ./config.toml stats -d research -n10
./nasfind --config ./config.toml index --folder /volume1/research/project --no-progress
```

建议将 DB 放在扫描根目录之外；如需放在里面，应在 `exclude_paths` 中排除其目录。
初次索引需要遍历文件名；局部合并也需要额外磁盘空间与本地主 DB 重建。
目录统计统一使用 `nasfind stats`，不再提供旧版 `dircount` 脚本，也不需要 Python。

## 安装到 PATH（可选）

```bash
sudo ./install.sh
nasfind init
```

安装器将准备好的 tools 一起复制到安装前缀的 `lib/nasfind/tools`。
不运行依赖准备脚本时，也可自行安装 plocate 和 GNU sort，然后使用系统工具；
自定义工具位置可通过 `[tools]` 配置覆盖。

## 定时更新与效率测试

群晖使用 DSM 的任务计划程序。建立用户自定义脚本任务，设置每两天运行：

```bash
/绝对路径/nasfind --config /绝对路径/config.toml index --no-progress
```

选择拥有扫描目录读取权限和 DB 目录写入权限的用户。安装包中的 systemd timer
用于普通 Linux，不应直接用于 DSM。基准测试步骤见 `BENCHMARK.md`。
