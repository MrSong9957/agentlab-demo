# 交付豁免：四向定向验证（2026-10-26，用户批准的限定例外）

背景：交付暂存扫描报 4 处命中，逐条核查均非凭据——3 处为 performance-final-025.json
内 benchmark 对源码文件的 SHA-256 完整性指纹（generic-api-key 形态误报），1 处为
mcp_tls 必需的自签测试私钥 src/app/tests/fixtures/tls/server.key。经用户明确授权，
作为"不扩大扫描豁免"的限定例外补两条**路径＋内容双锚定**豁免（`condition = "and"`），
未采用任意十六进制模式/PEM 头/整文件放行。

## 实现要点（gitleaks 8.30.1 实测）

- 旧版 `[allowlist]` 与 `[[allowlists]]$ 禁止共存（Failed to load config→exit 1）——
  既有值锚定豁免转为等价 `[[allowlists]]`（三个 2026-10-21 假样例值不变）。
- AND 语义字段为 `condition = "and"`（字符串；错误用法 `conditions = [...]` 会被静默
  忽略退化为 OR——二进制错误串 "unknown allowlist |condition| '%s' (expected 'and',
  'or')" 为证）。perf 块另设 `regexTarget = "line"`，regex 为三条完整 `"文件": "指纹"`
  字面（仅 `.` 转义）；key 块 regex 为 PEM 体全文（仅 `+` 转义）。

## 四向验证（隔离临时 Git 仓库，真实退出码，不碰真实暂存区；用后已清理）

命令形态：`gitleaks protect --staged --config .gitleaks.toml --redact`（与 hook 一致，
退出码直接取自 gitleaks，不经管道）。

| 用例 | 构造 | 预期 | 实测 |
|---|---|---|---|
| a | 授权路径＋授权内容（原四处命中＋配置自身入暂存） | 0 放行 | exit=0 ✓ |
| b | 同两文件各追加假密钥（sk-0123…hex32；AKIA…16 大写） | 1 拦截 | exit=1 ✓ |
| c | server.key 换另一把 openssl 现生成测试私钥 | 1 拦截 | exit=1 ✓ |
| d | 相同内容复制到未授权路径（server2.key；perf-copy.json） | 1 拦截 | exit=1 ✓ |

结论：仅"授权路径＋授权内容"同时满足才放行；同文件他值、他钥、同值异径均拦截。
其他规则与其他文件不受影响（值锚定块语义不变）。
