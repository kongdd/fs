# nasfind 原生版

下载与你的系统和架构对应的包，解压后即可使用，无需 Rust、Python 或 plocate。

| 系统 | 架构 | 包 |
|---|---|---|
| Linux（Ubuntu 26.04 验证） | Intel/AMD 64 位、ARM64 | linux-x86_64、linux-aarch64 |
| macOS 26+ | Apple Silicon | macos-aarch64 |
| Windows | Intel/AMD 64 位、ARM64 | windows-x86_64、windows-aarch64 |

Linux 包为静态可执行文件；Windows 包静态链接 C 运行库。不再提供 Intel Mac 包或验证 Ubuntu 24.04；macOS 要求 macOS 26 或更新版本；二进制未签名，首次运行可能需要在系统设置中允许。

## 开始使用

Linux / macOS：

```sh
./nasfind init
# 编辑 ~/.config/nasfind/config.toml，设置扫描目录和数据库路径
./nasfind index update
./nasfind search soil
./nasfind stats
```

Windows PowerShell：

```powershell
.\nasfind.exe init
# 编辑 $HOME\.config\nasfind\config.toml，路径建议写成 C:/data
.\nasfind.exe index update
.\nasfind.exe search soil
.\nasfind.exe stats
```

也可将可执行文件放到 PATH 中，直接使用 `nasfind`。数据库必须在扫描目录之外；已有 plocate/v1 数据库请改用新路径，不会自动转换。

默认 Rust 引擎，无需外部索引工具。支持 Unicode、Everything 表达式、子目录更新、统计和离线查询；Windows 输出路径使用 `/`。原 plocate 版的 `ignore`、`--dirs`、`--files`、`--mnt` 尚未接入此版本。

发布页附有 `SHA256SUMS.txt`，可校验下载包。
