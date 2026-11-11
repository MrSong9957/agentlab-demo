# A4/S27 验证记录（2026-11-11）

## 语义核实（改造前，逐输入域对比 lexical_normalize vs absolute_lexical）

| 输入域 | lexical_normalize | absolute_lexical | 结论 |
|---|---|---|---|
| RootDir | push "/"（字面量） | push MAIN_SEPARATOR（Unix 即 "/"） | Unix 逐字节一致 |
| CurDir/ParentDir/Normal | 忽略/pop/push | 相同 | 一致 |
| 领先 `..`、空串、相对路径 | pop 空栈得空串 | 相同 | 一致 |
| Windows Prefix（盘符/UNC） | 丢弃（缺陷：跨盘归一同形误判） | 保留（038 实测注释） | Unix 无此组件 |

结论：Unix 输入域两函数逐字节等价 → 可安全替换（无 Unix 侧差异，未触发阻断）。

## 改动

- apply_patch.rs：删除 lexical_normalize 及其文档与错挂孤儿文档（「末端符号链接
  拒绝的错误判定」——is_symlink_reject 残留，S8 暂缓项）；调用点改接
  `crate::sandbox::absolute_lexical`（单点），注释补 Windows 前缀保留说明。
- sandbox.rs：absolute_lexical 文档补 029 apply_move 消费方与分叉史。
- 029 规格 §1.4 末尾：登记修订（跨盘 Move 误判已修、单一实现点、Unix 语义不变）。
- sandbox/tests.rs：新增 unix 归一化单测（去点/顶层父级/空串，随 lib 跑）＋
  cfg(windows) 单测（盘符保留、跨盘不等、UNC 保留）。
  提交前修正（2026-11-11）：两新测移出「T1 策略 argv」文档注释作用域（该注释
  恢复挂接 policy_argv 测试），缩进对齐模块级风格；测试逻辑未变。

## 验证

- cargo check --tests：0 错误；警告 605＝基线零新增。
- lib apply_patch 38/0；absolute_lexical unix 单测 1/0；--test apply_patch 55/0；
  --test apply_patch_sandbox 10/0（真实 bwrap）。
- Windows cfg(windows) 单测与 Windows 行为：**未编译验证**（本机无 windows
  target，rustup 联网装组件此前失败）——静态核实 cfg 门控与绝对 lexical
  逻辑，待 038 CI Windows 腿（continue-on-error）顺带执行。

## 规格登记

docs/specs/029-file-patch.md §1.4 末尾修订段（2026-11-11，A4/S27）。
