# 规格：提交前密钥扫描

- 状态：待验收（2026-10-26 实现完成，证据 docs/logs/026-pre-commit-secret-scan/）
- 更新时间：2026-10-26

## 1. 要做什么，有哪些要求

为仓库建立提交前密钥扫描，阻止凭据明文再次进入提交。背景：真实密钥曾于仓库最早提交起被跟踪进入 Git 历史，2026-10-21 以 git-filter-repo 重写 48 提交并重建远端才完成处置；当前无任何提交前拦截（无 pre-commit hook、gitleaks 非常驻、`git-token-check.sh` 不识别 `sk-` 模型密钥形态）。

- 工具：复用 gitleaks（2026-10-21 已用官方发布、SHA-256 校验过的 8.30.1 验证过全历史扫描能力），对**暂存区实际内容**扫描（`gitleaks protect --staged` 或等价 git 模式）。
- 入库内容：`.gitleaks.toml` 规则（至少覆盖 `sk-[a-f0-9]{32}`、`ghp_/gho_/ghu_/ghs_/ghr_/github_pat_`、`AKIA[0-9A-Z]{16}`、PEM 私钥头）；固定测试样例**值锚定**豁免（2026-10-21 逐条核查的假值，见 `2026-10-21-github-sanitization.md` 表；2026-10-26 交付时转为 `[[allowlists]]` 等价形式）；交付追加两条**路径＋内容双锚定**（`condition = "and"`）豁免——performance-final-025.json 的三个源码 SHA-256 指纹（generic-api-key 误报）与 TLS 测试夹具 server.key（private-key），四向验证见 logs/`2026-10-26-delivery-exemptions.md`；不整体豁免任何目录；hook 脚本（`scripts/hooks/pre-commit`）；安装说明（含 gitleaks 二进制安装与 SHA-256 校验步骤）。
- 安装：尊重现有 `core.hooksPath`——已设置时提示手动放置，未设置时由安装命令复制／链接到 `.git/hooks/pre-commit`；安装动作不静默覆盖已有 hook。
- 失败语义：gitleaks 缺失、无法执行或扫描出错时**拒绝提交**（fail closed），提示信息只含形态与安装指引，不回显命中内容；命中时拒绝并显示文件与规则名。
- 绕过边界如实声明：本地 hook 可被 `--no-verify` 绕过；闭环需远端／CI 扫描，属后续独立决策，本规格只做本地层。
- 范围外：不扫描未跟踪文件（真实 `.env` 不在跟踪内，不被读取）；不新增对 `~` 家目录的扫描；不改远端。

## 2. 哪些情况容易出错

- 暂存区与工作区不一致：staged 的是干净版、工作区已改出密钥 → 扫描对象必须是暂存内容，行为按 staged 语义。
- gitleaks 不在 PATH → 静默放行等于没有防线 → fail closed。
- allowlist 写成目录级 → 未来测试目录新增真实密钥被豁免 → 仅精确文件＋规则。
- hook 覆盖用户已有 hook → 安装前检测并提示。
- 拒绝信息回显命中行 → 二次泄露 → 只报文件与规则名。

## 3. 重要做法，为什么这样做

- gitleaks 而非自写 grep：成熟规则引擎与 `--staged` 语义，且本项目已验证过其全历史扫描有效；自写脚本仅作 gitleaks 缺失时的失败提示，不作第二套检测逻辑。
- 规则级 allowlist + 假样例核查：豁免最小化，每次豁免都能指回 2026-10-21 的逐条核查记录。

## 4. 踩过的坑，怎样解决

- （实现中记录）

## 5. 先预测，再实测，最后确认是否合格

用假 Key 验收（全部为顺序字符等明显假值，不用真实值）：
- 暂存含 `sk-`＋32 位假十六进制文件 → 预测提交被拒、输出含文件名与规则名、无值 → 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- 移除该文件后提交 → 预测通过 → 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- staged 干净但工作区同路径文件含假 Key → 预测按 staged 语义通过（并如实记录该边界）→ 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- `git config core.hooksPath` 指向自定义目录后安装 → 预测不覆盖、给出手动放置指引 → 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- 卸载 gitleaks 后提交 → 预测 fail closed 拒绝并提示安装 → 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- 既有 5 处测试样例正常提交 → 预测不被误拦 → 实测：通过（2026-10-26 六场景，见 docs/logs/026-pre-commit-secret-scan/）。
- 交付豁免四向定向验证（隔离仓、真实退出码）→ 预期：原四处命中放行、同文件他值/他钥/同值异径拦截 → 实测：通过（2026-10-26-delivery-exemptions.md）。
- 证据：命令、输出（脱敏）存 `docs/logs/026-pre-commit-secret-scan/`。
- 验收：验收人按本节复跑；实现方状态最多"待验收"。
