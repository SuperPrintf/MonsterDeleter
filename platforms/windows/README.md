# Windows 适配

当前生产实现位于 `src/main.rs` 与 `installer/`。它使用 Win32 分层窗口实现透明动画覆盖层，并用 Inno Setup 注册资源管理器右键菜单。
