# 025 批次安全门禁收口（2026-10-26）

按 tdd → comprehensive-testing → Security Testing → code-review → verify 顺序执行：
- 025 脱敏：假凭据四项演练→真实 artifact 替换（sk=2 ghp=2）→全 ~/.zcode 复扫零明文→
  .env 内容不变（布尔 true）。全程无明文输出/备份。
- 026 扫描：gitleaks 8.30.1 官方校验安装；六场景实测（拒绝/通过/staged 语义/fail closed/
  hooksPath 兼容/值锚定豁免精确——豁免值与另一假 Key 同文件仍拦截且不回显）。
- 027 边界：提权实测（sudo NOPASSWD: ALL）、同 UID 0600 可读、ZCode 快照跨四根路径——
  终态阻断＋管理员操作清单（见 027 日志），未虚报完成。
- 022 TLS：三场景＋回环回归；错误证书拒绝与默认信任隔离实测。
- 028 依赖：rustls 0.23.46＋孤儿条目升级（webpki-roots 1.0.11/rustls-webpki 0.103.16），
  终态 cargo-audit exit 0、漏洞 0（5 个既有 unmaintained/yanked 警告如实保留）。
- 024 补测：TUI×anthropic×-i 初始图——请求含 base64 图片块（载荷逐字节比对）、TUI 响应流程完成；
  纯补覆盖，首跑失败为测试对 020 语义的误解修正（初始图自动提交、ctx 合并），非产品缺陷。
真实凭据零接触：无 Read/Edit .env；完整性仅布尔；无付费 API 调用。
