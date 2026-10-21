# 安全修复 F1/F2（2026-10-09）

依据：`docs/security/reports/2026-10-09-backend-security.md`（F1 .env 信任根劫持、F2 非常规文件 Update）。

- F1：`catalog::load_dotenv_from` 跳过 `TRUST_CONTROL_VARS`（5 个目录/地址入口，全库 env 读取点审计；`eq_ignore_ascii_case` 防 Windows 大小写绕过——独立 Review 必修项）。普通凭证变量仍可从 .env 加载；调用者显式 env 设置目录不受影响。
- F2：`apply_patch::apply_hunk` Update 分支 `fs::metadata` 判型（跟随链接、stat 不阻塞），非常规文件校验型拒绝；Add/Delete/悬空链接/跟随链接语义不变；残留竞态与存在性检查同级（注释载明）。
- 测试：新增 `dotenv_skips_trust_control_vars`（含小写变体）与 `f2_update_rejects_non_regular_targets`（子进程＋15s 看门狗，测试自身不挂起）。RED（断言失败/看门狗超时）→ GREEN（0.22s 明确报错）。
- 攻防复验（新编译候选）：F1 完整攻击链（有效指纹授权记录＋.env 劫持五变量）→ mock 0 请求、授权根留在家目录；正例 a/b（显式目录预授权、.env 普通凭证）凭证头实测正确。F2：FIFO 即时返回（此前挂起 124）、/dev/null 拒绝（此前假成功）；A/B/E 电池无回退。
- Review：独立子代理审查——F1 名单完整（含 ureq 无 proxy-from-env 核查）、必修 Windows 大小写绕过已修；F2 判型/语义一致、a1–a6 无误伤。
- 全量：`cargo check` 0 警告；`cargo test --workspace --no-fail-fast` 退出码 0，151 通过 / 0 失败 / 0 跳过。

## F2 收口（第二轮，同日）：判型与读写的对象替换窗口

- 修复：Update 读入改 `open(O_RDONLY|O_NONBLOCK|O_CLOEXEC)`＋fd fstat 判型＋同 fd 读入；写入经 `/proc/self/fd/<读fd>` 重开同一 inode（O_TRUNC 仅作用于已判型对象），重开后比对 `(st_dev,st_ino)`＋类型；/proc 缺失 fail-closed 报错。
- RED（确定性，受控窗口）：6M 行×6 万块（读入/计算≈2~3s），1s 时改名换入 FIFO / 原子 rename 替换链接为 FIFO——修复前两连看门狗超时（挂起），修复后 5s 通过且终态与替换时点无关。
- 行为保持：静态非常规拒绝、链接跟随、Add/Delete/悬空/权限/清单/失败状态/EFBIG 半写契约全部不变（全量回归＋readonly/a1–a6/second_file/d6 用例）。
- Review：必修项（变体二时点失控/间隙竞态→改受控窗口＋原子 rename）与两项纵深防御均落地；0444 经 /proc/fd 重开仍 EACCES（无提权）。
- 全量：`cargo check` 0 警告退出码 0；`cargo test --workspace --no-fail-fast` 退出码 0，152 通过 / 0 失败 / 0 跳过。

## F2 最终验收（第三轮，同日）：窗口测试确定性收口

- 同步屏障接缝：`#[cfg(test)] f2_seam`（ARMED＋before_write：reached/release 文件协调，60s 超时），调用点在判型＋读入＋计算后、写入前；生产构建零符号（strings 复核），env 仅测试代码读取。
- 确定性测试：`f2_swap_barrier_binds_write_to_validated_inode`（lib 单元测试，子进程 --exact 重入＋看门狗）；变体一（改名换 FIFO）/变体二（原子 rename 替换链接）在写前时点替换；0.07s 稳定。旧计时窗口测试移除。
- 变异证明（专用副本、显式 --target-dir）：写回 fs::write → 屏障测试红 15.01s（看门狗）；恢复绿、字节一致；Review 独立复证。
- Review：无必修；可选（变体二 state_known）已采纳。平台范围：Linux 容器（/proc/self/fd＋O_NONBLOCK 语义），如实声明。
- 全量：`cargo check` 0 警告退出码 0；`cargo test --workspace --no-fail-fast` 退出码 0，152 通过 / 0 失败 / 0 跳过（单元 71＋apply_patch 32＋exec 9＋llm_flow 40）。
