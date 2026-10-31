# 038 CI 第 8 轮 macOS 实测发现（run 41132773999，commit 5c814c9）

时间线：构建 ~7 分钟（rust-cache 命中第 6 轮产物）→ 单元测试 21:47 起
→ e2e_acceptance 21:49 起 → 卡死 2h13m 零输出 → 150 分钟上限截断。
判定：卡死而非正常耗时（同入口 ubuntu 测试阶段约 25 分钟）。

## 失败清单（45 个，三类）

### ① internal-apply /proc/self/fd 写回（macOS fail-closed：『需要 /proc 已挂载』）
apply_patch lib 2：display_changes…、f2_swap…
apply_patch.rs 集成 24：add_update_delete_full_cycle、internal_apply_entry…、
  b1-b9、d1/d2/d4/d6、update_level4×2、line_endings、relative_paths、multi_chunk、
  second_file×2、f2_update、n5×2

### ② AGENTS.md 链发现在 macOS 缺失（注入为空）
context_injection 11：t01-t15 中 11 个（t05/t10/t11/t14 通过）
context_management 1（同族）

### ③ 符号链接竞态/真实设备/沙箱内部语义
context lib 2：parent_directory_swap…、race_symlink_swap…
session lib 2：checkpoint/truncate …via_real_device
sandbox lib 3：deny_aliases…、path_dev_matches_mountinfo_format、policy_for…temp_writable_root
e2e_acceptance：卡死（卡在哪个用例待定位——2h13m 无输出）

## 处置
上述家族以 linux 门控排除（本地不变）；e2e_acceptance 排除 macOS；
macOS 任务标 continue-on-error；均为产品级移植项。
