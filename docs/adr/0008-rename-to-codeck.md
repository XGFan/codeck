# 改名 CoDeck

Claude Viewer 能 Connect Session（ADR-0007）之后，「Viewer」已不贴切；名字里的「Claude」也是 Anthropic 的商标。应用改名 **CoDeck**：code、co-deck（与终端共显——ccrpc 的 `--co-display`）、deck 三层意思。显示名写 CoDeck（大写 D，避免读成 codec），代码、包名、路径、环境变量里写小写 `codeck` / `CODECK_`。

改动范围：应用显示名与 bundle id（`dev.joy.claude-viewer` → `dev.joy.codeck`，即 Caches / Application Support / Logs 目录随之改变）、npm 与 Tauri crate 名（`codeck`）、索引 crate（`cv-core` → `codeck-core`）、环境变量前缀（`CV_` → `CODECK_`）、GitHub 仓库（`claude-code-viewer` → `codeck`）。`transcript` 与 `ccrpc` 不改名。应用里不写数据迁移：旧 bundle id 下的设置、索引、日志与 WebView 存储在改名时手动搬到新目录；没搬的话索引会自动重建，只丢失界面偏好。

## Considered Options

- **Codeck（显示名也小写 d）**：容易读成 codec。
- **ccdeck**：与 ccrpc 成一族、没有同名项目，但丢了 code 的双关。
- 已知同名：npm 的 `codeck` 已被占用，GitHub 上有几个同名的小项目（含一个 macOS 演示文稿应用）；本应用不发布到 npm，可以接受。

## Consequences

- ADR-0001～0007 保留当时的名字（Claude Viewer、cv-core），不回改。
