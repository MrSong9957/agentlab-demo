#!/usr/bin/env python3
"""033 验收 seccomp 补丁生成器：Docker 默认 profile → 最小放宽版本。

用法：033-seccomp-patch.py <默认profile.json> <输出.json>
来源：宿主机按 `docker version` 对应的 moby tag 下载 profiles/seccomp/default.json
（033-sandbox-accept.sh 自动完成并回显基线 tag；本脚本只做结构化修改，不下载）。

依据（bubblewrap **v0.9.0** 源码核对，2026-10-26）：
- 命名空间创建**仅一次**：bubblewrap.c:2900 `raw_clone(clone_flags, NULL)`
  （utils.c:866：x86_64/aarch64 上 flags 位于 arg0；s390/cris 参数反转——
  规则以 excludes arches 排除反转架构）。无 libc unshare() 调用。
- clone_flags 合成（bubblewrap.c:2842-2868）：`SIGCHLD | CLONE_NEWNS` 为基础，
  按选项 |= NEWUSER/NEWPID/NEWNET/NEWIPC/NEWUTS/NEWCGROUP。
- 产品实际产生**两种** clone 形态（2026-10-27 宿主首轮实测补充——初版只放行
  完整沙箱形态，probe_bwrap_at 干跑被 EPERM 拒，probe_available 恒 false）：
  1. 完整沙箱 argv（--unshare-user --unshare-pid --unshare-ipc）：
     SIGCHLD|CLONE_NEWNS|CLONE_NEWUSER|CLONE_NEWPID|CLONE_NEWIPC = 939655185
     （0x38020011；不含 NEWNET/NEWUTS/NEWCGROUP）。
  2. probe_bwrap_at 干跑 argv（--unshare-user --unshare-ipc，无 --unshare-pid）：
     SIGCHLD|CLONE_NEWNS|CLONE_NEWUSER|CLONE_NEWIPC = 402784273（0x18020011）。

放宽集（仅上述实际调用；全部前插，优先于默认规则）：
1. clone(flags) **精确等值** 939655185 与 402680849 各一条（masked_eq：
   value=掩码、valueTwo=期望值）——单一规则只放行一种调用形态；其余 clone
   组合走默认规则（无 ns 位放行/带位拒绝）；clone3 维持 ENOSYS 迫使 glibc
   回退 clone（bwrap 即走 clone）。
2. mount / umount2 / pivot_root 无条件 ALLOW —— 必要性：默认 profile 的
   mount/umount2 规则按 **init_user_ns** 判定 CAP_SYS_ADMIN（用户命名空间内
   进程不满足），pivot_root 无显式规则直接落 defaultAction EPERM（v27.5.1 实
   测核对）。安全性：seccomp 仅是过滤器，内核调用点仍执行 **ns 范围**能力检
   查（无能力进程放行后仍 EPERM）；放宽面＝内核 mount/pivot_root 代码路径对
   容器进程可达（与容器化 Chromium 沙箱同一权衡）。

不修改任何默认规则条目本身；不加 privileged/CAP_SYS_ADMIN/unconfined；
不放行 setns/chroot（本项目不需要——setns 仅 --userns-fd 选项使用）。
unshare 仅放行 flags 恰为 CLONE_NEWUSER 的一条等值规则（2026-10-27 宿主实测
补充）：--dev（devpts 挂载）场景下 bwrap 在第一级 userns 以 root 工作，
pivot 后必须 unshare(CLONE_NEWUSER) 建第二级 userns 恢复沙箱 uid
（bubblewrap.c:3239，实测 EPERM 报「unshare user ns」）；其余 unshare 组合
（NEWNS/NEWPID 等，需 init-ns CAP_SYS_ADMIN）维持默认拒绝。
util-linux unshare(1) 的其他用法同样不放行，验收脚本不以其作断言。
"""
import json
import sys

# 完整沙箱 argv：SIGCHLD|CLONE_NEWNS|CLONE_NEWUSER|CLONE_NEWPID|CLONE_NEWIPC
CLONE_FLAGS_EXACT = 939655185  # 0x38020011
# probe_bwrap_at 干跑 argv：同上无 CLONE_NEWPID
CLONE_FLAGS_PROBE = 402784273  # 0x18020011
# bwrap 第二级 userns（devpts 后 uid 恢复）：unshare(CLONE_NEWUSER)
CLONE_NEWUSER_ONLY = 268435456  # 0x10000000

INSERT = [
    {
        "names": ["clone"],
        "action": "SCMP_ACT_ALLOW",
        "args": [{
            "index": 0,
            "value": CLONE_FLAGS_EXACT,
            "valueTwo": CLONE_FLAGS_EXACT,
            "op": "SCMP_CMP_MASKED_EQ",
        }],
        "excludes": {"arches": ["s390", "s390x"]},
        "comment": ("033: bwrap raw_clone(CLONE_NEWNS|NEWUSER|NEWPID|NEWIPC|SIGCHLD)"
                    " - exact flags (bubblewrap 0.9.0 bubblewrap.c:2900; not unshare())"),
    },
    {
        "names": ["clone"],
        "action": "SCMP_ACT_ALLOW",
        "args": [{
            "index": 0,
            "value": CLONE_FLAGS_PROBE,
            "valueTwo": CLONE_FLAGS_PROBE,
            "op": "SCMP_CMP_MASKED_EQ",
        }],
        "excludes": {"arches": ["s390", "s390x"]},
        "comment": ("033: bwrap probe dry-run raw_clone(NEWNS|NEWUSER|NEWIPC|SIGCHLD)"
                    " - exact flags, no NEWPID (probe_bwrap_at omits --unshare-pid)"),
    },
    {
        "names": ["unshare"],
        "action": "SCMP_ACT_ALLOW",
        "args": [{
            "index": 0,
            "value": CLONE_NEWUSER_ONLY,
            "valueTwo": CLONE_NEWUSER_ONLY,
            "op": "SCMP_CMP_MASKED_EQ",
        }],
        "comment": ("033: bwrap second-level unshare(CLONE_NEWUSER) restoring "
                    "sandbox uid after devpts mount in 1st userns (bubblewrap.c:3239,"
                    " triggered by --dev; host-acceptance 2026-10-27 EPERM 'unshare user ns')"
                    " - exact flags, other unshare forms stay denied"),
    },
    {
        "names": ["pivot_root"],
        "action": "SCMP_ACT_ALLOW",
        "comment": ("033: bwrap pivot_root (bubblewrap.c:3109) - default profile has "
                    "no rule (EPERM); kernel capability check still applies at call site"),
    },
    {
        "names": ["mount"],
        "action": "SCMP_ACT_ALLOW",
        "comment": ("033: bwrap mounts (proc/tmpfs/devpts/bind, bubblewrap.c:1115-3169) - "
                    "default rule requires init-ns CAP_SYS_ADMIN; kernel ns-scoped "
                    "capability check still applies at call site"),
    },
    {
        "names": ["umount2"],
        "action": "SCMP_ACT_ALLOW",
        "comment": ("033: bwrap umount2(MNT_DETACH) for oldroot teardown - same "
                    "init-ns rationale as mount"),
    },
]


def main() -> None:
    src_path, dst_path = sys.argv[1], sys.argv[2]
    with open(src_path, encoding="utf-8") as f:
        profile = json.load(f)
    rules = profile["syscalls"]
    # 幂等：重复运行不重复插入。
    existing_comments = {r.get("comment", "") for r in rules}
    new = [r for r in INSERT if r["comment"] not in existing_comments]
    profile["syscalls"] = new + rules
    with open(dst_path, "w", encoding="utf-8") as f:
        json.dump(profile, f, indent=2)
        f.write("\n")
    print(f"inserted {len(new)} rule(s) at front; total rules "
          f"{len(rules)} -> {len(rules) + len(new)}")
    # 结构化差异（仅新增条目，不打印全量）。
    print(json.dumps(new, indent=2))


if __name__ == "__main__":
    main()
