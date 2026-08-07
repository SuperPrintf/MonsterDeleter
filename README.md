# MonsterDeleter（小怪兽删除器）

> **下载最新 Windows x64 安装包：** [MonsterDeleter-Setup-1.0.17.exe](https://github.com/SuperPrintf/MonsterDeleter/releases/download/v1.0.17/MonsterDeleter-Setup-1.0.17.exe)
>
> **选择历史版本：** [GitHub Releases](https://github.com/SuperPrintf/MonsterDeleter/releases)

MonsterDeleter 是一个 Windows 10/11 桌面端趣味删除工具：在资源管理器中右键点击文件、文件夹或快捷方式，选择“小怪兽删除”，小怪兽便会跑到目标旁确认并将选中的对象放入回收站。项目保留原始素材的动画、气泡按钮和音效体验，以 Rust 和原生 Win32 重新实现。

安装需要管理员权限，以便为全部用户注册资源管理器右键菜单；卸载可在控制面板的“程序和功能”中完成。

## 功能

- 右键菜单支持普通文件、文件夹和 `.lnk` 快捷方式。
- 半透明、逐像素透明的选择层，不遮挡桌面；红色准星跟随鼠标，`Esc` 可随时取消。
- 保留小怪兽进场、指向、气泡、确认按钮和对应音效；针对屏幕边缘、多显示器自动调整布局。
- 删除使用回收站语义，不调用 Windows 原生删除确认框。权限不足时先询问，再经 UAC 提权重试。
- 可选“卸载功能”：对符合规则且确实关联到已安装应用的 `.lnk` 或 `.exe`，展示第二层“小怪兽”询问，并仅启动该软件登记的官方卸载程序。
- 快捷方式解析支持 `.lnk → .lnk → .exe` 嵌套（最多 4 层，带循环保护）；无可用卸载程序时只允许“只删除”或“取消”。回收时始终处理用户选中的原始快捷方式，绝不删除其指向的可执行文件。
- 可选“卸载功能静默执行”：尽量向官方卸载程序传递静默参数；UAC 或厂商窗口仍可能出现。

## 程序结构与核心逻辑

```text
src/main.rs                    Rust/Win32 分层透明窗口、状态机、动画及回收站逻辑
assets/                        怪兽帧、气泡、准星、音效、程序图标
tools/bcu-bridge/              受限的卸载识别桥接器，只解析已安装应用的官方卸载记录
installer/MonsterDeleter.iss   Inno Setup 安装、卸载和右键菜单注册
docs/config.example.json       卸载功能配置示例
build-installer.ps1            构建桥接器、主程序并封装 Windows 安装包
dist/                          本地生成的版本化安装包（不提交源码仓库）
platforms/                     将来 macOS/Linux 平台适配预留
```

交互由有限状态机驱动：

```text
选择目标 → 透明选点层 → 怪兽进场/确认
                              ├─ 普通对象：删除动画 → 回收站
                              └─ 符合卸载规则的 .lnk/.exe：核验已安装应用
                                     ├─ 找到官方卸载程序：询问“需要卸载吗？”
                                     ├─ 仅识别到程序：询问“只能删除，无法卸载”
                                     └─ 非软件或不匹配：直接使用普通删除流程
```

选择层采用 Win32 分层窗口和 Alpha 合成；未绘制区域完全透明。卸载识别与启动使用解析后的 `.exe`，但删除步骤始终使用用户原始选择路径，因此不会把“删除快捷方式”错误变成“删除快捷方式指向的程序”。

## 配置卸载识别

安装后配置位于：`%LOCALAPPDATA%\MonsterDeleter\config.json`。完整示例见 [docs/config.example.json](docs/config.example.json)。

```json
{
  "uninstall": {
    "enabled": true,
    "mode": "official",
    "target_patterns": [
      "(?i)^.*\\.lnk$",
      "(?i)^.*\\.exe$"
    ],
    "cleanup_after_uninstall": false
  }
}
```

- `enabled`：是否启用删除确认后的二阶段卸载询问。
- `mode`：当前仅支持安全的 `official` 模式，即启动登记的官方卸载程序。
- `target_patterns`：匹配**文件名**的 Rust 正则表达式数组。默认覆盖 `.lnk` 与 `.exe`；例如仅检查快捷方式时可保留 `(?i)^.*\.lnk$`。
- `cleanup_after_uninstall`：预留项，当前保持 `false`，不会删除卸载后残留内容。

规则只决定哪些目标需要尝试卸载识别；即使匹配规则，程序也必须解析为有效 `.exe` 并关联到已安装应用，才会显示卸载询问。

## 编译

### 环境

- Windows 10 或 11 x64
- [Rust stable（MSVC 工具链）](https://www.rust-lang.org/tools/install)
- .NET 8 SDK（构建卸载桥接器）
- [Inno Setup 6](https://jrsoftware.org/isinfo.php)（构建安装包）
- PowerShell 5.1 或更高版本

### 主程序

```powershell
cargo test --offline
cargo build --release
```

输出：`target\release\monster-deleter.exe`。

### 安装包

```powershell
.\build-installer.ps1
```

脚本会先发布自包含的卸载桥接器，再构建 Rust 主程序并调用 Inno Setup。输出位于 `dist\MonsterDeleter-Setup-<版本号>.exe`。

## 发布与历史版本

源码仓库不提交安装包。每次正式发行都创建一个 Git 标签和对应的 GitHub Release，并将当次版本的 `MonsterDeleter-Setup-<版本号>.exe` 作为 Release 附件上传。旧 Release 与旧附件保持不变，因此用户可在 [Releases 页面](https://github.com/SuperPrintf/MonsterDeleter/releases) 按版本下载，而不会被新版本覆盖。

`platforms/windows`、`platforms/linux` 和 `platforms/macos` 为后续跨平台适配预留；动画状态机与业务策略会逐步下沉为平台无关的 Rust 核心。
