# fs

解压与你的系统、架构对应的包即可使用，无需 Rust、Python 或 plocate。

| 系统 | 架构 | 包 |
|---|---|---|
| Linux（Ubuntu 26.04 验证） | x86_64 / ARM64 | linux-x86_64 / linux-aarch64 |
| macOS 26+ | Apple Silicon | macos-aarch64 |
| Windows | x86_64 / ARM64 | windows-x86_64 / windows-aarch64 |

Linux 发布包静态链接；Windows 静态链接 C 运行库。macOS 二进制未签名，首次运行可能需要在系统设置中允许。不提供 Intel Mac 包或验证 Ubuntu 24.04。

Linux / macOS：

```sh
./fs init
# 编辑 ~/.config/fs/config.toml，设置扫描目录和数据库路径
./fs updatedb
./fs locate soil
./fs stats
```

Windows PowerShell：

```powershell
.\fs.exe init
# 编辑 $HOME\.config\fs\config.toml，路径建议写成 C:/data
.\fs.exe updatedb
.\fs.exe locate soil
.\fs.exe stats
```

Unix 可运行 `sh install.sh` 安装到 `$HOME/.local/bin`；或将可执行文件直接放到 PATH 中。数据库必须位于扫描目录之外。Windows 输出路径使用 `/`。

旧 `NASFIND_CONFIG`、`~/.config/nasfind/config.toml` 和原生 v2 数据库仍可使用；`index / search` 保留为别名。不自动转换 v1/plocate 数据库。发布页附 `SHA256SUMS.txt` 供校验。
