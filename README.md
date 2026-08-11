# MonsterDeleter（小怪兽删除器）

> **最新 Windows x64 安装包：** [MonsterDeleter-Setup-1.1.0.exe](https://github.com/SuperPrintf/MonsterDeleter/releases/download/v1.1.0/MonsterDeleter-Setup-1.1.0.exe)
>
> **历史版本与校验信息：** [GitHub Releases](https://github.com/SuperPrintf/MonsterDeleter/releases)

MonsterDeleter 是一个面向 Windows 10/11 的桌面删除工具。在资源管理器中右键文件、文件夹或快捷方式，选择“召唤小怪兽删除”，小怪兽会完成选点、进场、确认、爆炸和回收站删除的整套动画。它保留了原项目的怪兽素材、气泡、按钮和音效，并以 Rust 与原生 Win32 API 重构。

安装需要管理员权限，以便为全部用户注册资源管理器右键入口；卸载可在“控制面板 → 程序和功能”中完成。

## v1.1.0 重点更新

- 多选入口改为 Explorer COM 选区回调：一次获取完整选中项目，消除原先按文件启动、延时合并造成的启动迟缓和漏项。
- 多目标且未命中卸载功能时，只播放一次怪兽确认动画，并将所有原始选中项作为一个回收站操作处理。
- 多个可卸载目标会提供“逐一指定”或“全部卸载”分支；逐一模式使用准星高亮、固定叉标记和独立确认。
- 安装包包含新的资源管理器命令 DLL，并在升级时清理旧的逐文件引导器。

## 功能

- 支持文件、文件夹和 `.lnk` 快捷方式；快捷方式删除始终作用于 `.lnk` 本身，不会误删其指向的程序。
- 全屏覆盖层采用逐像素 Alpha 合成，未绘制区域保持透明；选点阶段使用原始准星贴图，按 `Esc` 可随时取消。
- 适配多显示器、DPI 与屏幕边缘：怪兽、气泡与按钮会在目标附近自动调整位置和朝向。
- 删除使用回收站语义，不调用 Windows 原生删除确认窗口；权限不足时会询问并允许通过 UAC 提权重试。
- 可选的软件卸载识别：仅当目标符合规则、能解析到有效程序且能在已安装软件索引中确认时，才进入第二层卸载询问。
- 卸载执行由受限桥接器调用软件自身登记的卸载程序；可选择“卸载功能静默执行”。

## 多目标与卸载逻辑

1. Explorer 将一次右键操作的完整选区传递给程序，程序只创建一个主动画流程。
2. 首轮气泡会询问“喂，是这些吗？”。按 `Esc` 或“取消”会终止本次操作。
3. 未启用卸载功能、没有可验证卸载项，或用户选择只删除时：所有原始选中项会作为一个批量操作放入回收站，并在每个可见目标位置绘制独立爆炸效果。
4. 单一可卸载目标：删除其它项目后，对该目标显示卸载询问。
5. 多个可卸载目标：可选择一次全部卸载，或使用准星逐一选择；每确认一个目标，界面会保留叉标记，最后再批量启动所选软件的官方卸载程序。

## 配置

安装后配置文件位于：`%LOCALAPPDATA%\MonsterDeleter\config.json`。也可从开始菜单的“Monster Deleter 设置”打开简易设置窗口。

```json
{
  "uninstall": {
    "enabled": true,
    "mode": "official",
    "target_patterns": [
      "(?i)^.*\\.lnk$",
      "(?i)^.*\\.exe$"
    ],
    "batch_target_patterns": [
      "(?i)^.*\\.lnk$"
    ],
    "cleanup_after_uninstall": false
  }
}
```

- `enabled`：是否在删除确认后启用软件卸载识别。
- `mode`：`official` 使用常规官方卸载流程；`silent` 表示尽可能传递静默参数，但仍可能显示 UAC 或厂商窗口。
- `target_patterns`：单目标卸载识别的 Rust 正则数组，默认匹配 `.lnk` 和 `.exe`。
- `batch_target_patterns`：多选时参与卸载识别的正则数组；默认仅 `.lnk`，避免普通文档、文件夹或可执行文件被意外纳入卸载流程。
- `cleanup_after_uninstall`：预留项，当前固定为 `false`，不会清理软件残留文件。

正则只决定哪些项目可参与检测；仍需解析到有效 `.exe`，并与已安装程序索引唯一匹配，才会触发卸载询问。

## 项目结构

```text
src/main.rs                    Win32 分层透明窗口、状态机、动画、音频、回收站与卸载流程
src/shell_extension.rs         Explorer IExecuteCommand COM 入口，接收完整多选数组
assets/                        怪兽、气泡、准星、音效和程序图标
tools/bcu-bridge/              受限卸载桥接器及其 BCUninstaller 许可信息
installer/MonsterDeleter.iss   Inno Setup 安装、卸载、COM 注册与右键菜单
docs/config.example.json       配置文件示例
build-installer.ps1            构建桥接器、Rust 程序和安装包
dist/                          本地生成的安装包（不提交源码仓库）
platforms/                     预留的 Windows / Linux / macOS 平台目录
```

## 编译

环境要求：Windows 10/11 x64、Rust stable（MSVC 工具链）、.NET 8 SDK、Inno Setup 6 和 PowerShell 5.1+。

```powershell
cargo test --offline --lib --bins
cargo build --release --lib --bins
```

构建完整安装包：

```powershell
.\build-installer.ps1
```

如桥接器未变化且本地已有 `assets\tools\bcu-bridge\bcu-bridge.exe`，可在离线环境中复用它：

```powershell
.\build-installer.ps1 -SkipBridgeBuild
```

输出位于 `dist\MonsterDeleter-Setup-<版本号>.exe`。

## 发布与历史版本

安装包不提交到 Git 仓库。每个正式版本都创建独立 Git 标签与 GitHub Release，并将对应安装包作为 Release 附件上传；旧版本不会被覆盖，用户可在 [Releases](https://github.com/SuperPrintf/MonsterDeleter/releases) 页面选择下载。
