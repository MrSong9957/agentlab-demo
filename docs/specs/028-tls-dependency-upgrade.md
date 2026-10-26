# 规格：TLS 依赖升级

- 状态：已交付本地 main（8df9278，2026-10-26）（rustls 0.23.46、webpki-roots 1.0.11、rustls-webpki 0.103.16，audit exit 0）
- 更新时间：2026-10-26

## 1. 要做什么，有哪些要求

在兼容范围内把 TLS 依赖链升级到无 advisory 版本，消除 cargo-audit 报警。审计基线（2026-10-25，cargo-audit 0.22.2，RustSec DB `21d909e` 2026-10-24）：6 漏洞＋5 警告。

已核实的事实链：

- 激活链＝`rustls 0.23.45 ← ureq 2.12.1 ← terminal-chat`（模型 API 客户端）；受 RUSTSEC-2026-0346/0347/0348/0349 影响，修复版本 `>=0.23.46`。
- lock 孤儿条目 `rustls-webpki 0.103.15`（>=0.103.16 修复 0382）与 `webpki-roots 1.0.9`（>=1.0.10 修复 0381）当前无引用者，但触发审计报警；022 修订（MCP HTTPS）启用 reqwest TLS 后依赖树变化，须在**终态**统一核对，不预设升一个包即全清。
- 警告项（bincode/zvariant 系 unmaintained、clipboard-win yanked）为 TUI 传递依赖、无利用面：仅记录，不做大版本升级。

要求：

- `cargo update -p` 将激活链 rustls 升至 >=0.23.46；孤儿条目升级到修复版本或随依赖树变化自然消除（以终态 `cargo audit` 实际输出为准）。
- 不做跨大版本升级；不通过扩大 allowlist／忽略配置掩盖报警；`Cargo.lock` 变更随本批提交入库。
- 与 MCP HTTPS（022 修订）同批终态核对：feature 启用后的新增 TLS 条目一并审计。
- git 依赖 rmcp（rev `98a5478`）不动；其与 crates.io 版本差异属工具覆盖限制，如实记录。

## 2. 哪些情况容易出错

- 先升级再启用 feature → 依赖树随后变化产生新报警 → 顺序上以终态审计为准（feature 先行或同批，最终统一复验）。
- 升级后 Cargo.lock 里仍留旧版本条目（被多个版本约束牵住）→ 核对激活链实际解析版本，不以 lock 中存在旧条目当作未修复。
- 把 unmaintained 警告当漏洞修 → 大版本升级引入不必要风险 → 仅记录警告项。

## 3. 重要做法，为什么这样做

- patch 级 `cargo update -p`：四个 advisory 的修复版本都在同 minor 内，无 API 变化，回归风险最低。
- 终态审计复验：依赖报警的"清零"只能以最终 `cargo audit` 输出为据，不逐包推断。

## 4. 踩过的坑，怎样解决

- （实现中记录）

## 5. 先预测，再实测，最后确认是否合格

- 升级后 `cargo tree -i rustls` 激活链版本 → 预测 >=0.23.46 → 实测：待测。
- 终态 `cargo audit`（同工具与 DB 或注明新 DB 版本）→ 预测漏洞 0（unmaintained/yanked 警告如实保留并列出）→ 实测：待测。
- `cargo check --locked --offline && cargo test --workspace --no-fail-fast` → 预测与基线一致（64 目标全绿，3 忽略）→ 实测：待测。
- TLS 行为回归：既有 https 相关测试（模型 API 层 mock 为 http，不覆盖 TLS 握手）→ 如实记录 TLS 握手无本地覆盖，由 MCP HTTPS 修订（022）的 TLS mock 测试补充覆盖 → 实测：待测（随 022 修订验收）。
- 证据：命令、lock diff 摘要、audit 输出存 `docs/logs/028-tls-dependency-upgrade/`。
- 验收：验收人按本节复跑；实现方状态最多"待验收"。
