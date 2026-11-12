# 批 5.2 安全测试进入/跳过依据（2026-11-11）

## 本轮触及的安全语义面

1. **A9（批 2）**：031 沙箱 view_image 传输编码——「主进程只接收有界编码图片」
   的边界声明。修改＝编码形态（十进制→base64）与接收上限数值（4/3×4 MiB＋
   4 KiB），**有界性保持**（确定上界）；prepare 层 4 MiB 原始上限不变——恶意
   JS 无法借新路径扩大读取或传输。覆盖：view_image lib 24/0（含往返无损锚）、
   e2e_sandbox 7/0（真实 bwrap 内部子进程路径）、view_image 集成 15/0。
2. **A10.5（批 3）**：V3 装载校验**加严**（截断片段 ≤64 KiB）——只收紧不放宽。
3. **A10.4（批 3）**：Windows raw_errno 恒 -1——消除 Win32 码域与 POSIX ELOOP
   理论撞码（提示分支确定不触发）。
4. **A8（批 2）**：合并链生效——输出条数语义，无权限/边界变化；
   js_orchestration_security 20/0 覆盖编排安全面。

## 结论

未新增攻击面；全部触及面有真实安全套件覆盖且全绿
（js_orchestration_security 20/0、security_attack_031 7/0、e2e_sandbox 7/0、
apply_patch_sandbox 10/0）。**专项 Security Testing 以受影响套件全绿代替
本轮全量攻防轮**——依据：修改均为边界收紧/编码澄清/平台正确化，无新授权
路径、无新解析面、无信任边界移动。
