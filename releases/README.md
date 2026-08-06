# 发布目录

此目录记录发布约定；可直接下载的安装包通过仓库的 [GitHub Releases](https://github.com/SuperPrintf/MonsterDeleter/releases) 页面托管，而不是提交到代码树。

- 当前发布资产：Windows x64 安装包。
- 未来可增加 Windows ARM64、Linux x64、macOS Apple Silicon 等发布资产。

日常构建输出位于根目录 `dist/`，并由 Git 忽略。正式发布时，为每个版本创建独立的 Git 标签和 GitHub Release，再将该版本的 `MonsterDeleter-Setup.exe` 上传为 Release 附件，并在 Release 说明中记录版本号、变更摘要和 SHA-256。

同名附件只会在同一个 Release 中被替换；不同 Release（例如 `v1.0.12`、`v1.0.13`）各自保存附件。因此不要删除或编辑旧 Release，历史安装包就会一直可供用户在 Releases 页面选择下载。README 中使用 `releases/latest/download/...` 指向最新版，同时提供 Releases 页面链接供选择历史版本。
