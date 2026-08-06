# MonsterDeleter（小怪兽删除器）

一个面向 Windows 桌面的趣味删除工具：在资源管理器中右键选择文件或文件夹，召唤小怪兽执行删除。它保留了原项目的动画、问答按钮与音效体验，并以 Rust 和原生 Win32 窗口重构。

> 当前仅支持 Windows 10/11 x64。安装包需要管理员权限，以便为所有用户注册资源管理器右键菜单。

## 功能

- 在资源管理器文件和文件夹的右键菜单中提供“小怪兽删除”入口。
- 以半透明、不遮挡桌面的选取层让用户点选目标；红色准星跟随鼠标。
- `Esc` 随时取消选取或动画流程；在确认按钮出现前不会删除任何内容。
- 小怪兽走入、指向目标、展示气泡与“是的 / 嗷嗷嗷就是这个”交互按钮，并播放原始素材音效。
- 针对屏幕边缘和上下空间自动调整怪兽、气泡与按钮位置；多显示器时只使用鼠标所在屏幕。
- 删除通过回收站语义执行，不展示系统删除对话框。遇到权限不足时，先清楚询问；确认后通过 UAC 提权重试。
- 安装时为全部用户写入右键菜单；卸载时移除关联和安装文件。

## 程序结构与核心逻辑

```text
.
├─ src/main.rs                       # Rust/Win32 透明覆盖层、状态机、删除与提权流程
├─ assets/                            # 原始怪兽帧、对话气泡、准星、音频、程序图标
├─ installer/MonsterDeleter.iss       # Inno Setup：安装、右键注册、卸载
├─ build.rs                           # 将怪兽头部图标写入主程序资源
├─ build-installer.ps1                # 先编译，再调用 Inno Setup 生成安装包
├─ releases/windows/x86_64/           # 可直接分发的当前 Windows 安装包
├─ platforms/                         # 未来 macOS/Linux 的系统集成预留说明
├─ .github/workflows/windows-release.yml # 标签构建与 GitHub Actions 制品发布
└─ 原始项目文件/                       # 仅本地归档，不会提交到仓库
```

程序不依赖通用 GUI 框架绘制全屏窗口，而是使用 Win32 的分层窗口与逐像素 Alpha 合成：未绘制的像素保持完全透明，因此桌面始终可见，且不会产生黑屏或整屏纯色覆盖。

交互由一个有限状态机驱动：

```text
选取目标 → 淡出选取层 → 怪兽入场/指向 → 用户确认 → 删除动画 → 回收站删除
                 └────────────────── Esc：立即退出，不执行删除
```

在删除阶段，程序先以当前权限尝试放入回收站；若操作被拒绝，才出现自定义提权询问。用户同意后由 Windows UAC 启动受控的提权重试进程。安装脚本仅注册本程序自身的绝对路径，避免右键菜单命令被外部输入拼接或劫持。

## 编译

### 环境

- Windows 10 或 11，x64
- [Rust stable（MSVC 工具链）](https://www.rust-lang.org/tools/install)
- [Inno Setup 6](https://jrsoftware.org/isinfo.php)（仅生成安装包时需要）
- PowerShell 5.1 或更新版本

### 生成程序

在项目根目录运行：

```powershell
cargo build --release
```

生成文件：`target\release\monster-deleter.exe`。

### 生成安装包

```powershell
.\build-installer.ps1
```

脚本会先执行 release 构建，再调用 Inno Setup，输出 `dist\MonsterDeleter-Setup.exe`。安装时选择“为所有用户安装”会触发 UAC；这是写入系统级右键菜单所必需的。

当前可分发版本已纳入仓库：[MonsterDeleter-Setup.exe](releases/windows/x86_64/MonsterDeleter-Setup.exe)。

## 发布与跨平台规划

`releases/<系统>/<架构>/` 存放经验证、可直接下载的安装包；当前目录中的 Windows x64 安装包是首个基线版本。后续正式版本建议通过 GitHub Release 的附件分发，并以版本标签（如 `v1.1.0`）触发自动构建。仓库中的 GitHub Actions 会保留每次标签构建的安装包制品。

`platforms/windows`、`platforms/linux` 与 `platforms/macos` 预留了各平台入口和打包方式。动画状态机、素材编排和删除策略应逐步下沉为平台无关 Rust 核心；各平台目录仅实现对应的文件管理器菜单、透明覆盖层、权限请求和安装包适配。这样新增平台时不会破坏 Windows 版本的行为。

## 本地归档

`原始项目文件/` 是原仓库的本地参考副本，已被 `.gitignore` 排除，不会上传到 GitHub 或发行包中。
