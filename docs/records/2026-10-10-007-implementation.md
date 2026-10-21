# 007 会话保存与恢复：实现与验收证据（2026-10-10）

分支 `dev/007-session-persistence`（基线 0d1bb80），未提交工作区改动。规格：`docs/specs/007-session-persistence.md`（第六批定稿）。

## 复用与适配清单（规格 §3 取材顺序）

| 项 | 来源 | 方式 |
|---|---|---|
| 历史类型与 serde tagged 表征 | 本地 `src/prompt.rs` | 最小适配：`HistoryRole/HistoryMessage/ToolCallRecord/HistoryItem` 加 serde 派生（`{"type":"text"/"tool_calls"/"tool_result",…}`），字段与投影语义不变（缺失清单 1） |
| 私有目录/权限收紧 | 本地 `src/catalog.rs` | 直接复用 `user_dir()`、`ensure_private_dir`（0700）；快照 tmp 0600 |
| 快照原子替换 | 本地 `catalog.rs write_atomic` 模式 | 最小适配：独立 tmp（pid/纳秒/序号）＋流式逐记录写＋rename＋失败仅清 tmp（适配点：sessions/ 目录、逐记录而非整串） |
| 有界行读 | 本地 `api.rs LimitedReader` 模式 | 语义复用：行缓冲 64 MiB＋1 上限、超限转 64 KiB 块丢弃模式（新写为按行而非按体） |
| 独占锁 | 直接依赖 `libc` | `flock(fd, LOCK_EX|LOCK_NB)`（母本 `writer_lock.rs` 文件锁先例的本地简化；进程消亡自动释放） |
| 追加/提交点/装载容差 | Codex rollout（`recorder.rs`） | 语义采纳、本地新写：std 同步 File O_APPEND、每记录一次 `write_all`、无 fsync、无后台写者；跳坏行＋计数 |
| 配对修复 | Codex `normalize.rs` | 语义采纳、本地适配：合成通用未知错误结构（006 未知工具取消先例的形状），禁言"未执行/取消" |
| 确认交互 | 本地 `src/main.rs` | 直接复用 `prompt_y_n`＋授权门非交互拒绝先例 |
| 会话文件层/装载校验/REPL 接线/注入提供者 | 新写 | 缺失清单 2–5：`src/session.rs`＋`main.rs`/`turn.rs` 接线＋`TCHAT_SESSION_TS/PID` 注入 |

## TDD 记录（RED→GREEN）

- Slice A 序列化：先写 `serialization_representation_roundtrip`（编译失败＝RED）→ 加派生 GREEN。
- Slice B 装载修复：14 项先于实现编写（模块缺失编译失败＝RED）→ 实现 `load_from_reader/repair_pairs` GREEN。
- Slice C 写入器：10 项先写（API 缺失＝RED）→ 实现 `Session`/`save_snapshot` GREEN；实现中修复两个真实缺陷：
  1. 退出排空失败被"打印一次"纪律吞掉（flush_final 与 on_history 叠加过滤器）→ 拆分 `flush_attempt`（原始消息）＋on_history 施加打印纪律；
  2. ToolResult 保留路径漏推入 out（配对修复丢结果）。
- Slice D 接线：全量既有 218 项＋新增全绿。
- Slice E CLI E2E：13 项；排查记录见下。
- Slice F 性能：1 项综合（48 s 运行）。

## E2E 排查记录（关键坑）

1. PTY 测试 send 参数重复 `\n`（send 自身补换行）→ 每条输入多一空行、确认流错位。修复后另给 PtyCli wait 加 consumed 游标（只在新输出匹配）。
2. 会话文件表征是**平铺** `{"id","name","arguments"}`，与协议嵌套 `{"id","type","function":{…}}` 不同——e10 种子误用协议格式导致 tool_calls 行被跳过。
3. 管道模式无 `你> ` 提示符、重复运行时审计静默（manual.json 保持 0600）→ 就绪标志改横幅＋短沉降。
4. RLIMIT 受限 CLI 首轮会写 auto_catalog.json（配置初始化）→ 受限运行前先无限制运行一次 `/models` 预初始化。
5. **64 MiB 历史替换后验证轮必然被 10 MiB 请求上限本地拒绝**（006 t8 语义，Spec 审查发现）→ 行为验证改用 1 MiB 小会话替换后进行。

## 变异测试（AGENTS.md §4：证明能发现错误）

| 变异 | 内容 | 结果 |
|---|---|---|
| M1 | 合成文案违反禁词（"未执行"） | `dangling_calls` 杀死 |
| M2 | 半写后不截回 durable_len | `half_write` 杀死 |
| M3 | 被拒批不再充当分组边界 | **存活**→修正测试（伪造场景需被拒批结果 id 恰与前批悬空调用相同）后杀死 |
| M4 | 快照不 rename（无原子替换） | `save_*` 杀死 |
| M5 | user 记录入史点不挂钩 | e3 杀死 |
| M6 | 头部非 user 不丢弃 | `leading_non_user` 杀死 |
| M7 | 降级路径修改原文件（锁顺序违规） | e9 杀死 |

## 双轴审查（code-review skill）发现与整改

Standards 轴：
- PTY 基础设施三份重复 → 提取 `tests/common/pty.rs`（session_cli/session_perf 共用；e2e_acceptance 原有份未动）。
- `examples_parse_check_main.rs` 遗留调试脚本 → 删除。
- 行上限检查×3、包含性围栏×2 重复 → 提取 `line_over_limit/line_limit_error/ensure_within_dir`。
- 注释与断言矛盾（9/8 字节）→ 修正。
- `config/auto_catalog.json` updated_at 漂移为**任务开始前已存在**的本地副作用，未动（报告声明）。

Spec 轴（全部整改）：
- 替换验证轮设计错误（见排查 5）→ 已修。
- **`.env` 可注入 `TCHAT_SESSION_TS/PID` 破坏文件名时序** → 加入 `TRUST_CONTROL_VARS`（F1 机制扩展，7 项）。
- **/resume 确认丢弃前未排空旧待写段**（落盘集合≠内存集合）→ `cmd_resume` 在切换前 `flush_final`，失败中止恢复（e16 回归）。
- "修复一致性·重新装载"缺续写闭环 → 补 `repair_consistency_across_continuation`。
- 1 MiB 档吞吐单次采样 → 改 3 次中位。
- create_auto_file 名称冲突 `-N` 后缀重试为规格外防御性新增（保留，无害，注入同名场景需要）。

## 安全验证（Security Testing Gate）

- 权限：sessions 0700/文件 0600（库级＋e1 双验）。
- 路径边界：名称 charset/长度/auto- 保留、canonicalize 包含性、软链接逃逸拒绝（e15，目录外文件零触碰）、非法名称零文件操作（e11）。
- 输入校验：meta 门（唯一整拒）、行 64 MiB 上限、批内/跨批重复 id、行内 JSON 结构（serde 严格）。
- 资源限制：超限停用不截断、超长行 64 KiB 有界丢弃、10 MiB 请求上限独立生效（排查 5 亦为其证据）。
- 数据保护：不主动写鉴权配置（e1 断言凭证值不在会话文件）、不新增脱敏、用户输入/工具输出原样保留、输出经 `say`（display 边界）过滤。
- F1 扩展：`TCHAT_SESSION_TS/TCHAT_SESSION_PID` 入信任根跳过名单。

## 性能实测（容器内 /workspace，2026-10-10）

`perf-obs: gen=12.1s B0=6.4MiB H32=40.5 H64=74.5 H256=278.5 u=1.064 long-peak=270.6MiB thr1=11.1MiB/s thr256=121.1MiB/s append1=0.0126s append256=0.0099s sessions=10ms peaks=[109.1×5]`

- u≈1.06（≈1×文件字节＋少量解析开销）→ 无双份全量缓存的结构证据；256 档断言余量充足（272.1 ≤ 400.4）。
- 吞吐 256 档 121 MiB/s ≥ 1 MiB 档 11.1×0.5；追加耗时比 256/1≈0.78 ≤2×；/sessions 百文件 10 ms ≤50 ms；替换峰值 5 次恒定 109.1 MiB（无逐次累积）。

## 验证命令与结果

- 容器内 `/workspace`：`cargo check` 通过（0 警告）；`cargo test --workspace --no-fail-fast` 最终全绿（数量见完成报告；基线 218 项无回归）。
- 测试构成：库/单元＋集成（session 28、session_cli 14、session_perf 1、其余既有目标）。

## 未测与限制（如实声明）

1. ~~截回失败→停用分支仅代码审查保障~~ → **已闭合（收尾第二批，见下节）**：/dev/full 真实设备注入。
2. e9"写者半行暂停"以测试进程扮演暂停写者（持锁＋半行字节状态等价），非 CLI 进程内暂停接缝（保留）。
3. ~~并发 /save 末者胜未单测~~ → **已闭合（收尾第二批，见下节）**。
4. Windows 路径与权限未实现未实测（沿 catalog 既有口径）。

## 收尾补验（2026-10-10 第二批）

### 1. 截回失败停用（真实设备路径）

- 测试：`src/session.rs` 内嵌 `session::tests::truncate_failure_disables_autosave_via_real_device`。
- 注入方式：`/dev/full`（实测：write 恒 ENOSPC、ftruncate 恒 EINVAL）——真实字符设备 I/O 错误，
  经生产 `on_history` 路径触发"写失败→截回失败→停用"分支；无需生产接缝（模块内单测构造
  Writing 态指向该设备，前两条视为已提交）。
- 验证：报错含"截回失败/自动保存已停用"；停用后追加静默；`flush_final` 返回 Ok 证明已停用
  （未停用则会再次触达 /dev/full 失败返回 Err）；内存历史增至 5 条（对话继续）；残档形态
  （已提交行＋未提交半行）装载按坏行跳过、tail=Bad。
- 定向变异：把 `Err(te) => Disable(...)` 改为按 Pending 处理（不停用）→ 测试失败（报错文案
  断言先杀、flush_final Ok 断言兜底）；还原后通过。
- RLIMIT/chattr 不可注入的事实保留在前文排查记录。

### 2. 并发快照（末者胜）

- 库级：`concurrent_save_last_complete_snapshot_wins`——受控顺序 A→B→A2 最后成功替换者（A2）
  字节精确生效；双线程 Barrier 同发同名快照 ×10 轮：最终文件恰为某一方完整快照
  （期望字节独立构造）、无 tmp 残留、双方均 Ok。
- CLI 级：`e17_two_process_concurrent_save_same_snapshot`——两个真实 CLI 进程各恢复约 1.1 MB
  历史（1500 条）后同时 /save 同名：最终文件与某一方种子字节精确一致、装载完整（1500 条）、
  无 tmp 残留。
- 顺带修复测试设施存量缺陷：`LivePipe::finish` 原从已被读线程持有的 stdout `take()` 导致
  返回空输出——改为持读线程 JoinHandle、join 后取共享缓冲。

### 3. 性能口径（原始值与公式核对）

新鲜运行（2026-10-10，容器 /workspace，退出码 0，测试 1 项通过）：

```
perf-raw: B0_runs=[6656,6584,6584] H32_runs=[41536,41416,41252] H64_runs=[76032,76064,76128] H256_runs=[285040,285164,285204] (KiB)
perf-obs: B0=6.4MiB H32=40.4 H64=74.3 H256=278.5 u=1.057 ... thr256=123.5MiB/s sessions=13ms peaks=[109.1×5]
perf-formula(spec §6-J): u=max((H64-H32)/32,0)=(74.28-40.45)/32=1.0574; assert H256-B0=272.05 <= u*256+128=398.69 -> true
```

- 中位数：B0=6584 KiB（6.43 MiB）、H32=41416 KiB（40.44）、H64=76064 KiB（74.28）、
  H256=285164 KiB（278.48）。逐次采样离散 <1%。
- **规格公式**（§3/§6-J 第六批）：u=max((H64−H32)/32 MiB, 0)=1.0574；断言
  H256−B0=272.05 ≤ u×256+128=398.69 ✓。测试代码、原始数据、规格实测列三者一致。
- 单档斜率交叉验证：(H32−B0)/32=(40.44−6.43)/32≈1.063，与两档差 1.057 相近——线性良好、
  无双份缓存的结构证据。
- **/goal 给出的核对公式** `u = max((VmHWM(32) − B0 − 128 MiB)/32 MiB, 0)` 与规格公式不一致：
  按其计算 u'=max((40.44−6.43−128)/32,0)=0，则断言 H256−B0=272.05 ≤ 128 不成立；且对任何
  忠实实现都不可满足——装载 256 MiB 会话的历史驻留（规格 §3"历史驻留是必要开销，不得虚称
  常量内存"）本身 ≥256 MiB，u'=0 时断言等价于要求全部驻留塞进 128 MiB 余量。规格 §6-J 亦
  明文"128 MiB 仅作验收余量出现在断言式、绝不在校准中扣除"。**未改公式、未改阈值，提请
  裁决**（见完成报告）。


## 合并前高风险补测与门禁整改（2026-10-10 第三批）

### RED→GREEN（新增保护先证失败）

1. **write_atomic 排他创建＋清理守卫**（范围 5）：
   - RED：`preexisting_tmp_path_is_protected`/`preexisting_tmp_symlink_is_not_followed`——原实现对预置 tmp 路径静默覆盖/写穿成功（行为级 RED，接缝 TEST_TMP_PIN 为中性测试设施先行加入）。
   - 实现：`create_new` 排他（不复用路径、不跟随软链接）；`created` 标志——仅本次成功创建的 tmp 进入失败清理（open 失败直接返回，不动已存在路径）。
2. **grant_origin 原子写**（范围 5）：
   - RED：`grant_write_failure_preserves_old_store`——原 write_private_file 原地截断写直接成功。
   - 实现：`ensure_private_dir`＋`write_atomic`（目录 0700、最终 0600、失败保留旧授权文件）；rename 失败错误信息补终路径上下文。
3. **同名 auto 恢复**（范围 4，合并门禁）：
   - RED：`same_name_resume_keeps_original_as_append_target`——自持锁误降级"另一进程编辑"（注意：新创建的 auto 文件不持锁，场景仅出现于二次同名恢复）。
   - 实现：resume 同名时先释放旧描述符（双侧 canonicalize Some 且相等才判同名）再走正常"打开即锁"；**自释放后的首个 Busy 做有界微重试（3×10ms）**——根因取证：WSL2 内核 close→flock 释放滞后（变异实验：Busy 分支丢态后 100ms 重试即获锁；/proc/locks 检查时已无持有者）；第三方真实竞争重试耗尽后仍按 §6-I 降级，语义不变。

### 正确行为补测（无 RED，按设计断言）

- e18：S 按序列化层次二次解码三处逐字一致（工具结果/会话文件/请求2 tool 消息）；C（配置凭证）不入会话与请求正文、认证头正常；精确计数（2 请求、请求1=2、请求2=4 messages、5 行含 meta、0600）。
- e19：未授权两分——已有 auto 字节不变零新增；尚无 auto 不创建文件零入史。
- n4c：中文 UTF-8 字节 L（允许恰一请求）/L+1（拒绝零请求）两点（n4b 纯 ASCII 缺口补齐）。
- e21：超限 CLI 闭环——零请求无重试、user 已落盘（＋1 行）、/save N+1、/sessions、新进程再恢复；前提自证（投影独立重算 >L——文件字节≠投影字节，行→消息每条省 ~15B）。
- e22：同文件字节异投影——甲全有效拒（0 请求）；乙 1.5MiB 坏行跳过后恰一请求，逐消息比对＋固定开销 ≤4KiB。
- e23：注入精确文件名；无注入＝真实时钟 UTC 秒区间＋实际子进程 pid；非法形状回退。
- dotenv_skips_session_trust_vars：TS/PID 大小写/空值/重复全跳过、进程值保留。
- e24：占位替换失败——错误、零请求、占位原样、无 tmp 残留、零新增记录。
- e25：RLIMIT 正字节部分写两状态——探针先证"写过正数字节后失败"；(a) 无旧目标失败后仍不存在下次重询；(b) 旧含 A 字节不变、A 放行 B 重询；零请求零记录。
- e26：双写者并发授权——均成功持久化放行；终态无条件可解析、完整配对、无未同意、无 tmp 残留。
- broken_store_consent_replaces_with_single_pair：坏文件同意后整文件覆盖恰 1 对。

### 定向变异（检测力）

MUT-A 清理守卫移除→冲突测试杀；MUT-B3 同名释放移除→同名测试杀（生产微重试不掩护——变异时无 released_self 标志）；MUT-C grant 回退原地写→原子写测试杀。

### 审查发现与整改（独立子代理）

硬违规 2（已修）：n4b `#[test]` 被 n4c 插入吞掉（死代码，恢复属性）；TEST_TMP_PIN 锁域不交（tmp_conflict 并入 mod tests 共用 env_lock，8×压测全绿）。
判断性（已处理）：同名判定双侧 Some 收紧＋微窗口文档；e26 概率性说明；e25(b) 补零记录断言；死代码清理。
**测试设施 flake 根治**：d6/f2 偶发＝EFBIG/F2 子进程继承 chdir 用例瞬时 CWD（目录删后 ENOENT）→子进程显式 current_dir(/tmp)；perf 追加比率噪声底 10ms（无争用中位 10-13ms；比率阈值不变）；flock 假 EAGAIN＝WSL2 close 释放滞后→生产侧自释放微重试＋flock 测试基座迁 /dev/shm（取证链完整保留于上文）。

### 最终验证（verify:full）

- `cargo check`：退出码 0，0 警告。
- `cargo test --workspace --no-fail-fast`：退出码 0，**连续三轮 279 通过 0 失败**（263 基线＋本轮 16 项新增）。

### 文档同步

- 规格实测：同名恢复待验证→已验证（含修复与"新创建不持锁"注记）。
- 安全报告：2026-10-05 报告补充章节预测→实测结论全部补记。
- AGENTS.md：同名恢复＋授权原子写一句。
- critical-functional-test-cases：执行证据补记（AUTH-E02 原子写契约闭合等）。

## 会话写锁闭合与性能口径修正（2026-10-10 第四批）

### 偶发锁失败根因重判（修正第三批结论）

第三批曾结论"flock 假 EAGAIN＝WSL2 内核 close→flock 释放滞后"，并以生产侧微重试＋测试基座迁 /dev/shm 作为"根治"。第四批受控实验推翻该根因：

- **实验 A（否定"close 释放滞后"假说）**：单线程 100,000 轮 `open+flock+close+立即 open+flock`，/tmp（overlayfs）与 /dev/shm（tmpfs）上 Busy 均为 **0**——在受测条件（该两文件系统、单线程热循环、无并发 spawn）下未见任何释放滞后；不外推为对全部内核/文件系统/并发形态的保证。
- **实验 B（fd 副本机制实锤）**：持锁 fd 经 fork 继承后，父进程 close，锁仍被持有（Busy=1）；fork 后关闭 3 以上全部 fd 再 close 父引用，立即释放（Busy=0）。
- **真根因（有可靠证据支持）**：测试二进制并行线程 spawn 子进程（探针/CLI）时，fork→exec 窗口内子进程持有写者 fd 副本（flock 锁挂 open file description，引用不灭锁不释）。第三批旧实现"同名恢复＝释放自持锁→重开→抢锁"恰在此窗口读到 Busy；其"100ms 微重试后成功"实为等子进程 exec（CLOEXEC 关闭副本）后自然成功——被误判为"内核滞后"；"/proc/locks 无持有者"快照摄于子进程退出之后（一次快照不能证明当时无引用）。overlayfs 假说同被实验 A 否定（/tmp 上零 Busy）。
- **修正声明**：第三批"WSL2 内核问题已澄清""flock flake 已根治（迁 /dev/shm）"两结论证据不足，予以撤回——迁 /dev/shm 只是降低了与探针测试的并发概率，未消除机制。另：更早一批 40 轮"全净"压测因取证插桩缺 `use std::os::unix::fs::MetadataExt` 编译失败、实际零测试运行，不得计为有效压测（第三批已报废，此处正式记录）。
- **剩余未知**：fork→exec 窗口的精确时长未测（估百微秒级）；生产 CLI 单线程 REPL 无"释放再重抢"序列，不受该窗口影响（见下"锁生命周期"）。

诊断程序：`/tmp/diag_flock.c`（三实验源码，一次性取证工具，不入仓库）。

### 锁生命周期核对（生产代码全路径）

持锁 fd 的产生、持有与释放路径（唯一写者 `Session`，`src/main.rs:43` 创建、五个入史点经 `on_record` 回调 `main.rs:273`、退出 `main.rs:76`、/resume 排空 `main.rs:191`、续写 `session.rs resume`）：

1. **新建路径**（本轮补锁）：`create_auto_file` `create_new` 打开 → **立即 flock** → 写 meta＋历史 → `State::Writing` 持 fd。Busy＝名字被并发恢复者抢先 → 换下一候选名（50 次耗尽停用）；Failed → 停用。
2. **恢复路径**：open → flock → 持锁装载/尾置/游标 → `State::Writing`。Busy → drop 新开 fd（未持锁）→ 降级只读（旧不同名 Writing fd 同时 drop＝放弃旧目标，CLI 层此前已排空待写段）。
3. **同名恢复**（本轮改复用）：不释放不重开，同一 fd rewind 重装载＋尾置＋游标重置——锁全程不离开写者，Busy 分支不可达。
4. **释放点（按代码实际枚举，共四类）**：①进程退出（fd 全关，锁自动释放）；②自动保存停用（行超限/截回失败/创建失败→`State::Disabled`，fd drop）；③**会话切换**——`resume` 到手动快照（`state=Idle` drop 旧 Writing fd）、`resume` 到不同名 auto 获锁成功（`state` 替换 drop 旧 fd）或降级（`state=Idle` drop 旧 fd），三处均为放弃旧目标＝释放其锁（CLI 层 `cmd_resume` 在切换前已排空旧待写段，失败即中止）；④同名恢复不释放（本轮改为复用描述符，锁不离开写者）。工具执行 spawn 子进程：fd 带 CLOEXEC，fork→exec 窗口内副本存在但父进程始终持锁且从不"释放再重抢"——窗口对生产无影响。
5. 结论：**所有持续追加 auto 的路径（新建/恢复/同名恢复三入口＋追加/截回/排空）均持锁执行**，与规格 §6-I 一致。

### 新建 auto 写者互斥（RED→GREEN）

- RED：`new_auto_writer_lock_excludes_second_writer`——A 新建 auto-X 保持写入状态，B `/resume` X 直接获锁续写（旧实现新建路径无锁，双写者并行成立），断言"B 必须降级"失败。
- GREEN：上述"新建即锁"。
- 场景价值：无参 `/resume` 扫 `latest_auto` 即命中写者进行中的文件（坑009 的真实触发面），非理论构造。
- 极端竞争注记（剩余可用性限制，未修）：A create_new 与 flock 之间若被并发恢复者抢先持锁，留下的 0 字节文件使抢锁者装载得到"会话文件为空"明确错误；A 换名继续。可用性影响：对该 stem 的**显式** `/resume` 会持续报错（0 字节文件不会自愈）、`/sessions` 列表可能多一项；**无参** `/resume` 按实现推断不受影响（真文件名带 `-N` 后缀，残留 stem 是真文件 stem 的前缀、字典序更小，`latest_auto` 取字典序最大必得真文件）——该推断无行为测试验证，**标未验证**。50 次持续 Busy → 停用。本轮不追加清理逻辑。

### 同名恢复复用描述符（消除释放窗口）

- 实现：`resume` 同名判定（双侧 canonicalize Some 且相等）后，同一 fd 上 rewind → 装载 → 尾置（AcceptedNoNewline 补 \n／Bad 截断）→ 游标重置；失败时原状态不变。第三批的"释放→重开→微重试"整体删除。
- **生产微重试删除理由**：其防御对象（自释放后伪 Busy）随"不释放"而结构上不存在；保留会把真 Busy（第三方）误拖延 30ms 且掩盖诊断。生产单线程无 fork 窗口自竞争。
- 受控竞争验证（`same_name_resume_survives_concurrent_lock_poacher`）：高频抢锁线程（open+flock 循环，得手即计数释放）全程下，写者 200 轮同名恢复无一降级、**抢锁者得手恒 0**、每轮重装载＝内存历史、全程未另建 auto 文件——用竞争事件证明锁从未离开写者，而非仅观察恢复前后两次锁状态。

### 变异检测力（本批四个变异，均先失败后还原复绿）

- **MUT-1**（移除 create_auto_file 的 flock）：`new_auto_writer...` 失败于"B 不得获得续写权限"。
- **MUT-2**（同名恢复改回"释放置 Idle→走通用打开即锁"）：竞争测试第 9 轮即被挤成降级；对带微重试的第三批实现，`得手==0` 断言同样可杀（得手即违反独占，与写者是否自愈无关）。
- **MUT-3a**（Busy 映射为 Acquired，假获锁）：`flock_conflict` 相位 1 降级断言失败。
- **MUT-3b**（获锁后立即 LOCK_UN，锁泄漏）：竞争测试"抢锁者得手过"失败。
- 测试侧重试（`resume_expect_acquired` 100ms 重试、flock 相位 3 进程内判定）**整体删除**——结构上不存在"第一次违反续写契约被重试洗白"的路径；相位 3 改为独立子进程判定（探针 `resume_lock` 场景，spawn 发生于释放之后、验证者 fd 表不含目标文件），同时免疫测试进程并发 spawn 的 fork→exec 窗口干扰。flock 基座自 /dev/shm 迁回 /tmp（真实文件系统，实验 A 证明与文件系统无关）。

### 性能口径修正（追加耗时比）

- **问题**：第三批为 CLI 轮询观测引入 10ms 噪声底（`a1.max(1e-2)`，从 1e-4 两度放宽而来），规格 §3 目标阈值①从未有过该下限——它使分母低于 10ms 的比率判断被实质放宽（"阈值仍为 2"不等于契约未变）。
- **修正**：改库级纯追加路径累计计时——每档 `/resume` 后连续追加 100 条 ~1KiB 记录（≤8 KiB 规格口径），总耗时/100 为单条值；直接计时 `Session` 追加路径（待写段序列化＋write_all），无轮询/管道/模型请求回路噪声；**无噪声下限**；同时断言批次落盘字节恰＝行字节总和（行为性 O(新增)）。
- **实测（2026-10-10）**：per1=0.000042s、per256=0.000046s、**比率 1.089 ≤ 2**；B0=6.4MiB、u=1.059、H256=278.6 ≤ B0+u×256+128=405.5（公式与阈值未动）；吞吐 120.7 ≥ 17.0×0.5；/sessions 8ms。原始值随 `perf-append/perf-raw/perf-obs/perf-formula` 输出。

### 最终验证（第四批复验）

- `cargo check`：退出码 0，0 警告。
- 锁相关定向压测（/tmp 基座，`-- flock same_name new_auto concurrent` 过滤）：连续 10 轮全绿。
- `cargo test --workspace --no-fail-fast`：退出码 0，**连续三轮 281 通过 0 失败**（第三批基线 279＋本轮新增 2 项：`new_auto_writer_lock_excludes_second_writer`、`same_name_resume_survives_concurrent_lock_poacher`）。
- 全量并行下探针/CLI 子进程 spawn 与锁测试同批运行——旧偶发机制（fork→exec 窗口 fd 副本）在新结构（无"释放再重抢"、相位 3 子进程验证）下无暴露面，三轮全量＋10 轮定向均无偶发；按第 3 节标准，本轮不宣称"环境级 flake 永不再现"，但已知机制均已消除且检测断言无放宽。

### 独立增量审查（第四批，独立子代理）

总裁决：**通过，无硬违规**。审查覆盖最终锁实现（三入口持锁、同名复用失败不半更新、Busy 分支丢态安全性、O_APPEND 与截断交互）与测试重试逻辑（无重试洗白路径、竞争断言能区分"从未离开"与"离开过但自愈"、探针相位 3 免疫 fd 副本窗口、测试间无污染）与性能口径（规格阈值、无噪声下限、字节恰增、计时无污染）。判断性问题 6 项：5 项已处理（0 字节残留措辞修正如上、resume doc 补排空前置、Busy/快照分支补 `error_reported` 复位对齐（CLI 路径不可达、库级防御）、日志数值笔误 385.5→405.5、perf 注释 1 KiB→2.4 KiB）；1 项记录为剩余限制（perf 的 `TCHAT_SESSION_TS` 注入值非 15 位、静默回退真实时钟，属既有死代码、对断言无影响，不在本轮权限内顺带修改）。

### 交付材料补齐（2026-10-10 第四批，仅文档）

#### 诊断实验可复现记录（源码此前仅存 /tmp，现收入日志）

环境：WSL2 内核 `6.18.33.2-microsoft-standard-WSL2`；/tmp＝overlayfs、/dev/shm＝tmpfs；cc 11.x。编译运行：`cc -O2 -o /tmp/diag_flock <源码> -lpthread && /tmp/diag_flock /tmp && /tmp/diag_flock /dev/shm`。三实验核心（完整源码的等价最小内核）：

```c
// A：单线程热循环，close 后立即再锁——检测释放滞后
for (long i = 0; i < 100000; i++) {
    int fd = open(path, O_RDWR|O_APPEND);            // 新 open file description
    flock(fd, LOCK_EX|LOCK_NB);                      // 获锁
    close(fd);                                       // 唯一引用关闭
    int fd2 = open(path, O_RDWR|O_APPEND);
    if (flock(fd2, LOCK_EX|LOCK_NB) != 0 && errno == EWOULDBLOCK) busy++;
    close(fd2);
}
// B：fork 继承（inherit=1）与 fork 后关闭 3 以上全部 fd（inherit=0，模拟 CLOEXEC）对照
int fd = open(path, O_RDWR|O_APPEND); flock(fd, LOCK_EX|LOCK_NB);
pid_t pid = fork();
if (pid == 0) { if (!inherit) for (int f = 3; f < 4096; f++) close(f); usleep(300000); _exit(0); }
close(fd); usleep(20000);                            // 父进程释放唯一显式引用
int fd2 = open(path, O_RDWR|O_APPEND);
busy = (flock(fd2, LOCK_EX|LOCK_NB) != 0 && errno == EWOULDBLOCK);
// C：8 线程各自 open+flock+立即 LOCK_UN+close 循环 1 万次（正常竞争基线）
```

结果（两文件系统一致）：A `busy=0`；B `inherit=1 → busy=1`、`inherit=0 → busy=0`；C busy 率 0.49（/tmp）/0.60（/dev/shm）。

#### 变异验证明细（本批四个有效变异＋一次无效记录）

| 变异 | 修改点（临时，均已还原） | 失败测试与实际断言 | 还原证据 |
|---|---|---|---|
| MUT-1 | 删除 `create_auto_file` 打开后的整个 `match try_flock` 块 | `new_auto_writer_lock_excludes_second_writer` panicked："A 持锁写者存在时 B 不得获得 X 的续写权限" | `cp` 备份还原后 session 31/31 |
| MUT-2 | 同名恢复复用块整体替换为 `self.state = State::Idle;`（重现"释放再重抢"窗口） | `same_name_resume_survives_concurrent_lock_poacher` panicked："第 9 轮同名恢复被挤成降级——锁在恢复中离开过写者" | 同上 |
| MUT-3a | `let lock = match try_flock(&file) { Busy => Acquired, other => other }`（Busy 伪装获锁） | `flock_conflict_degrades_to_readonly_then_resumable` panicked 于降级断言（`assert!(info.degraded)`） | 同上 |
| MUT-3b | `Acquired` 分支开头插入 `flock(fd, LOCK_UN)`（获锁即放） | 竞争测试 panicked："抢锁者得手过——写者的锁存在离开窗口"（`poached==0` 断言） | 同上 |

无效记录（不计入有效验证）：MUT-3a 第一版写成 `LockOutcome::Busy if false =>`，造成 match 非穷尽**编译失败**，无测试运行——按规则废弃后改用上表写法重做。每次变异均以独立备份还原并重跑通过；最终态 `cargo check` 0 警告＋全量 281/0（含整改后确认轮）。第三批的 40 轮编译失败取证循环同属无效验证，已在根因重判节正式报废。

#### 性能记录精度（追加耗时比，引用既有输出，未重跑）

- 操作数量：1 MiB 档与 256 MiB 档各 1 次 `/resume` 装载（装载不计时）＋**连续追加 N=100 条**（行 ≈2.4 KiB，规格口径 ≤8 KiB）；计时对象＝100 条 `on_history` 总时长＋末次 `flush_final`（无待写段，无操作）。
- 每次时长（打印值，`{:.6}` 即微秒级舍入）：per1＝0.000042s（42μs）、per256＝0.000046s（46μs）。
- 累计时长：测试只打印均值，总时长未单独输出——由打印值反推 1 档 ≈4.2ms、256 档 ≈4.6ms（推算值）。
- 比率：打印 1.089（三位小数舍入）；真实比率由打印舍入区间（42μs∈[41.5,42.5]、46μs∈[45.5,46.5]）得 **1.071–1.121**，均远低于阈值 2。
- 内存断言（完整式，单位 MiB）：`H256 − B0 ≤ u×256 ＋ 128` → 实测 278.6 − 6.4 ＝ 272.2 ≤ 1.059×256 ＋ 128 ＝ 399.1（等价全式 278.6 ≤ 6.4＋1.059×256＋128＝405.5）✓。B0/u/H256 原始三次采样见 `perf-raw` 输出行（日志第二批与测试 `--nocapture` 运行可得）。

### 本地交付验证（2026-10-10，Git 交付阶段）

候选提交 `83dd796`（dev/007-session-persistence，19 项任务文件，+5403/−32；范围外 `config/auto_catalog.json`、`db/` 未提交未还原）。隔离干净 worktree（`git worktree add --detach /tmp/007-verify 83dd796`，无未提交文件与未跟踪资源，`CARGO_TARGET_DIR` 指向独立目录避免污染共享产物）验证已提交内容：

- `cargo check --locked`：退出码 0，0 警告。
- `cargo test --workspace --no-fail-fast --locked`：退出码 0，**通过 281 / 失败 0 / 忽略 0**（实际计数，与工作区三轮基线一致）。
