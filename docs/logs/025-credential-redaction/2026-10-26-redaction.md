# 025 凭据脱敏执行记录（2026-10-26）

脚本：/tmp/025-redact.py（形态匹配就地替换，保留 kind/toolName/toolCallId/createdAt/version/files[].path；
JSON 校验失败不写入；输出仅计数）。假凭据演练四项通过：替换（sk=1 ghp=1）、幂等（二次 0/0）、
字段保留（fields-ok）、异常路径（FileNotFoundError→exit 3）。

真实执行（~/.zcode/cli/artifacts/sess_d8f6c5bf…/call_0b74d2d3…-tool-result-f1c28c62….json）：
- replaced sk=2 ghp=2；remaining 0/0；metadata=kept（独立 jq 级核验通过）。
- 占位符 [REDACTED-sk-2026-10-26] / [REDACTED-ghp-2026-10-26]。

全 ~/.zcode 复扫：sk-hex32 明文 0 处、ghp 系明文 0 处。
/workspace/.env 内容未变（前后 SHA-256 相等，仅输出布尔）：true。

代码侧脱敏出口对照（仅修已证实缺口）：
- api.rs 错误脱敏（先脱敏再截断）＋redact_data_urls：002/019 既有契约，覆盖模型 API 错误出口（024 已测）。
- mcp/secrets.rs：连接期 sanitize_owned 先于存入 server.error、调用期各 CallFailure 出口、stderr tail
  （2026-10-25 补查逐点核实）。
- display.rs sanitize_for_terminal：终端净化（012 契约）。
- git-token-check.sh：GitHub token 专域（vault 侧）；sk- 形态的提交拦截缺口由 026（gitleaks）覆盖，
  本仓库无新增缺口需修改既有代码。
结论：目标 1–4 全部达成；未创建任何明文备份；未使用 Agent Edit 接触 artifact。
