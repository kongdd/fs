# fs

解压与你的系统、架构对应的包即可运行，无需 Rust 或 Python。默认使用 Rust 引擎，数据库可在各系统间复制。只有 Linux 可显式改用 plocate。

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
./fs updatedb --engine rust
./fs locate soil
./fs stats
```

Windows PowerShell：

```powershell
.\fs.exe init
# 编辑 $HOME\.config\fs\config.toml，路径写成 C:/data 或 C:
.\fs.exe updatedb --engine rust
.\fs.exe locate soil
.\fs.exe stats
```

Unix 可运行 `sh install.sh` 安装到 `$HOME/.local/bin`；或将可执行文件直接放到 PATH 中。数据库可以放在扫描根目录之内，建库时会跳过数据库文件、锁文件和 SQLite 旁路文件。Windows 输出路径使用 `/`。

默认引擎是 `rust`。Linux 可用 `fs config engine plocate` 改用 plocate；其他系统只有 `fs config engine rust`。也可临时用 `FS_ENGINE`。优先级为 `--engine` > `FS_ENGINE` > 配置文件 > 内置默认 `rust`。切换默认值不会转换已有 DB。

旧 `NASFIND_CONFIG`、`~/.config/nasfind/config.toml` 配置入口仍可使用；`index / search` 保留为别名。**原生数据库仅支持当前 schema v4，旧 v1/v2/v3 库须使用新的数据库路径重建，不自动迁移或覆盖。** Unix 可显式使用 plocate 后端，其数据库不自动转换为原生格式。

目录变更检测使用秒级时间戳，同一目录在一秒内发生的多次变化可能漏检。根目录在数据库中保存一次，其他目录路径相对编码；查询支持离线使用和多个同格式索引。

发布页附 `SHA256SUMS.txt` 供校验。
