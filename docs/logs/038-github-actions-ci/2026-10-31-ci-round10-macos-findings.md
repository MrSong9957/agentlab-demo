# 038 CI 第 10 轮 macOS 实测发现（run 41151715764，commit 37e0942）

时间线：构建 ~5 分钟（缓存命中）→ 测试活动 00:48-00:53（5 分钟，3195 通过/49 失败）
→ native_model_apis 卡死 2h21m → 150 分钟截断。

## 与第 8 轮合并的 macOS 缺口全景（产品级移植项）

1. PTY 驱动测试系统性卡死（2 处实证：e2e_acceptance 第 8 轮、native_model_apis
   第 10 轮；两轮卡死点不同＝逐用例门控不可穷尽，需产品级 PTY 层移植）；
2. /proc/self/fd 写回（internal-apply，macOS fail-closed）：apply_patch 集成＋lib 共 26；
3. AGENTS.md 链发现缺失：context_injection/context_management 12；
4. 符号链接竞态/真实设备/沙箱内部（bwrap//proc/mountinfo）：lib 7；
5. 本轮新暴露（在第 8 轮未到达的目标中）：mcp/multi_agent/js_orchestration_security/
   memory/proc_session/message_to_user 等家族共 ~40 失败（明细见 Actions 日志）。

## 处置
macOS 任务 continue-on-error（结果可见、不阻塞 ubuntu）；150 分钟上限为卡死防线；
两类移植（Windows 进程语义层、macOS /proc 与 PTY 层）为产品级后续工作，
CI 已交付其可见性与证据。
