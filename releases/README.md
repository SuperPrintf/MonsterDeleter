# 发布目录

此目录记录发布约定；可直接下载的安装包通过仓库的 [GitHub Releases](https://github.com/SuperPrintf/MonsterDeleter/releases) 页面托管，而不是提交到代码树。

- 当前发布资产：Windows x64 安装包。
- 未来可增加 Windows ARM64、Linux x64、macOS Apple Silicon 等发布资产。

日常构建输出位于根目录 `dist/`，并由 Git 忽略。正式发布时将该文件上传为 GitHub Release 附件，并在 Release 说明中记录版本号、变更摘要和 SHA-256。这样用户能在 Releases 页面一键下载，源代码页面也不会显示大体积二进制文件。
