![Header](assets/MiPCMPatcher.png)

# MiPCM Patch

为「小米电脑管家 / 小米互联 / 超级小爱」提供功能增强、安装辅助与还原能力的补丁工具。支持图形界面与命令行两种使用方式。

![GUI 预览](assets/MiPCM_GUI.png)

## 能做什么

| 功能 | 说明 |
|---|---|
| 🗺️ **地区伪装** | 让小米电脑管家读取指定的地区值（默认 `CN`），不修改系统真实地区 |
| 📷 **抑制摄像头误报弹窗** | 屏蔽「摄像头暂不可用，点击确定打开设备管理器」这类本机摄像头被误判禁用的弹窗 |
| 🔊 **音频流转增强** | 在无线 WiFi 与有线 LAN 之间切换音频流转的网络介质 |
| 💻 **设备伪装** | 伪装为指定机型，解锁机型相关功能 |
| 📤 **Windows 11 右键小米互传** | 在文件和文件夹一级右键菜单中启用/关闭「使用小米互传发送」 |
| 📦 **安装电脑管家 / 小米互联** | 选择电脑管家、互联 Windows 版或内测版，一键下载并启动安装；也可手动指定链接或本地安装包 |
| 📦 **安装超级小爱** | 一键下载并安装，安装完成后向实际版本目录部署专用 `userenv.dll`（不限制版本） |
| 📊 **状态查看** | 检查当前安装位置、各补丁状态 |

每个功能均提供还原路径，补丁操作按当前状态执行并尽量保持幂等。

## 下载

从 [Releases](../../releases) 页面下载最新版本：

- `MiPCM_Patch_GUI_v*.*.*.exe` — 图形界面（推荐）
- `MiPCM_Patch_CLI_v*.*.*.exe` — 命令行工具

## 快速开始

### 图形界面

双击 `MiPCM_Patch_GUI_v*.*.*.exe`，在弹出的 UAC 窗口中点击「是」，即可看到补丁操作界面：

1. 点击「刷新」重新读取当前安装与补丁状态
2. 按需点击各功能的「应用」按钮
3. 已启用的功能会禁用重复操作，并开放对应「还原」按钮

安装区提供「一键安装 / 手动安装 / 卸载」：

| 安装来源 | 版本 / 使用要求 |
|---|---|
| 小米电脑管家 | 5.8.1.130 |
| 小米互联 OS3 | Windows 版 1.1.2.36，配合 Xiaomi HyperOS 3 Beta 及以上手机 |
| 小米互联 OS4 | Windows 内测版 2.0.2.524，配合 Xiaomi HyperOS 4 及以上手机/平板 |
| 超级小爱 | 3.5.0.220 |

小米电脑管家与小米互联互斥：检测到其中一个已安装后，两个产品的安装入口都会禁用。超级小爱的安装状态独立。电脑管家和小米互联分别使用独立网址与本地文件入口，选错产品会拒绝安装并记录日志。点击「手动安装」才会展开下载地址和本地 `.exe` 选择。下载完成后校验固定 SHA-256 并启动安装窗口，请按窗口提示完成安装；超级小爱安装窗口退出后自动注入补丁。

内测版下载地址由官方滚动更新，本工具只启动与内置 SHA-256 一致的已核验安装包。官方更新后若校验失败，需更新工具的下载来源配置，或自行下载并通过手动入口安装。

下载使用内嵌 aria2，最多八路连接，显示完成比例、已下载大小、总大小和速度。服务器不支持分段时使用单连接；实际速度仍取决于下载源和网络。可取消下载，再次下载相同地址时续传。下载缓存位于 `%TEMP%\MiPCManager_Patch\downloads` 下的来源隔离子目录，可随 Windows 临时文件清理；自动清理取决于存储感知设置。缓存保留时，已完成的推荐版会在重新校验后复用，未完成的下载可以续传；缓存被清理后需要重新下载。自定义地址的完整同名文件不会被覆盖，可通过本地文件入口安装。

### 命令行

```text
# 查看状态
MiPCM_CLI.exe status

# 地区伪装
MiPCM_CLI.exe locale apply
MiPCM_CLI.exe locale revert

# 抑制摄像头弹窗
MiPCM_CLI.exe camera apply
MiPCM_CLI.exe camera revert

# 音频流转
MiPCM_CLI.exe audio apply --mode lan     # 有线模式
MiPCM_CLI.exe audio apply --mode wifi    # 无线模式
MiPCM_CLI.exe audio revert

# 设备伪装
MiPCM_CLI.exe device apply --model TM2425
MiPCM_CLI.exe device revert

# Windows 11 右键小米互传
MiPCM_CLI.exe share-menu apply
MiPCM_CLI.exe share-menu revert

# 安装小米电脑管家 / 小米互联
MiPCM_CLI.exe install --recommended
MiPCM_CLI.exe install --recommended continuity
MiPCM_CLI.exe install --recommended hyperconnect-beta
MiPCM_CLI.exe install
MiPCM_CLI.exe install --installer "D:\path\to\installer.exe"

# 安装超级小爱并注入补丁
MiPCM_CLI.exe xiaoai install --recommended
MiPCM_CLI.exe xiaoai install
MiPCM_CLI.exe xiaoai install --installer "D:\path\to\XiaoaiAgent_Setup.exe"
MiPCM_CLI.exe xiaoai install --url "https://example.com/XiaoaiAgent_Setup.exe"

# 维护已安装的超级小爱补丁
MiPCM_CLI.exe xiaoai apply
MiPCM_CLI.exe xiaoai revert
```

无参数运行会进入交互菜单。

TUI 的「安装」面板按 ↑↓ 选择内置安装来源，按 Enter 下载安装；按 X 安装工具同目录中的超级小爱安装包，下载时按 C 取消。

内嵌下载器的来源、固定哈希和许可见 [aria2 NOTICE](assets/aria2/NOTICE.md)。Release 附带 `aria2-1.37.0-sources.zip`，提供下载器及静态链接库的对应源码和许可；运行工具无需另外安装 aria2。

## 常见问题

<details>
<summary>为什么需要管理员权限？</summary>

补丁需要修改 `C:\Program Files` 下的文件，右键菜单功能还需要注册 Shell Extension，因此需要管理员权限。Release 版 exe 已内嵌管理员权限清单，双击运行时会自动弹出 UAC。
</details>

<details>
<summary>补丁后需要重启电脑吗？</summary>

小米电脑管家相关补丁通常不需要重启，补丁后手动重新打开小米电脑管家即可。安装、应用或还原超级小爱补丁后，请按程序提示重启电脑；程序不会自动重启。右键小米互传在应用/还原时会刷新资源管理器相关状态。
</details>

<details>
<summary>有线音频流转出现重复设备怎么办？</summary>

在手机端将该电脑「移除 / 忘记」后重新配对即可。
</details>

<details>
<summary>只安装了「小米互联 / 互联互通」能用吗？</summary>

「小米互联 / 互联互通」(PcContinuity / HyperConnect 2.0) 支持**地区伪装**；Windows 11 右键小米互传功能独立于完整版电脑管家的摄像头、音频和设备伪装补丁。摄像头弹窗、音频流转、设备伪装等功能需要完整版「小米电脑管家」(XiaomiPCManager)。
</details>

<details>
<summary>操作失败提示“拒绝访问”怎么办？</summary>

工具会自动关闭对应进程并重试一次。如果仍然失败，请手动关闭小米电脑管家相关进程后重试。
</details>

## 技术说明

补丁原理、定位方式、实现细节与构建说明详见 [TechnicalIntroduce.md](TechnicalIntroduce.md)。

## 致谢

- Coolapk @Na1veMagic：地区伪装实现思路
- @WWW4R4E : 的 [WWW4R4E/Mi-transfer-station](https://github.com/WWW4R4E/Mi-transfer-station) 的 CecilDll 为摄像头 Patch 做了基础
- @ChsBuffer ：设备伪装所用 `msimg32.dll`
- @FarMounTAI : 超级小爱专用 `userenv.dll`
- @cnbluefire：Windows 11 小米互传 Shell Extension 原始实现（MIT）；本项目集成的精简载荷来自 `YYplus/XiaomiShareShellExt-Minimal` v1.0.2

## 免责声明

本工具所用图标归北京小米移动软件有限公司所有，受相关版权法律保护。未经授权，禁止任何形式的复制、分发、展示或使用这些图标。

本工具仅供学习和研究使用，作者不对因使用本工具而导致的任何直接或间接损失承担责任。使用者应自行承担使用本工具的风险，并确保其行为符合当地法律法规。
