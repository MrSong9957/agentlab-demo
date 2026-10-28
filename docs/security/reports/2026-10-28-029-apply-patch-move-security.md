# 安全攻防测试报告：029 apply_patch Move 与第四级 Unicode 匹配

- 日期：2026-10-28
- 被测对象：`src/app/src/apply_patch.rs` 的 `apply_move`（移动/重命名）与 `seek_sequence` 第四级（`map_unicode_punct` 28 码点映射）；沙箱面经 `apply_patch_sandboxed_with_exe → tool_launch_argv → bwrap → --internal-apply` 真实入口
- 规格：`docs/specs/029-file-patch.md`（§1.2/§1.3/§1.4）；既有边界：004/027/033
- 方法：先代码审查定位攻击面，再以真实二进制真刀真枪打一遍。非沙箱面用 `terminal-chat --internal-apply`（stdin 传补丁）；沙箱面用探针调用生产入口 `apply_patch_sandboxed_with_exe`（真实 bwrap，TCHAT_SANDBOX=1）。全部使用独立临时目录 `/tmp/sec029/` 与假数据（FAKE-SECRET），未触碰真实用户文件与仓库（测试脚本：`/tmp/sec029/{lib,a_path,b_link,c_race,d_resource,f_unicode,g_h,e_sandbox,e2}.sh`）
- 执行身份：uid 1000(dev)，容器 Linux，bwrap 可用

## 结论概要

8 类攻击矩阵全部实测。**核心防线全部有效**：词法等价拒绝、O_NOFOLLOW 源判型、目标排他创建（30/30 竞态抢占全拒）、删源前 inode 复核（真实竞态中拦截实证）、1 MiB/4 MiB/100k 行上限（一处被 lone-CR 绕过，见 F-1）、`.env` 遮蔽与沙箱越界拒绝、fail-closed 启动检查、四级匹配优先级与映射不写回、部分成功语义（不虚报 add、不回滚、state_known 区分）。

共确认 **1 个中危**（行数上限绕过）、**1 个低危纵深防御缺口**（procfs 伪文件经 Move 物化）、若干低危/信息项。无高危。

---

## 发现清单（按风险排序）

### F-1【中】lone-CR 文件绕过 100000 行读入上限（内存/工作量放大 ~10 倍）

- 后果：行数上限是规格 §1.6/A9 明文资源边界；`logical_lines` 用 `str::lines()`（仅按 `\n` 分行），而 `SourceFile::parse` 把孤立 `\r` 也当行尾。仅含 `\r` 的文件以"1 逻辑行"通过行数门，随后按真实行数（可达 ~200 万行）展开，峰值内存实测 165 MiB（2 MiB 输入），并违背"超限不得写入目标"的边界意图。字节数上限（4 MiB）仍有效，故为有界 DoS。
- 复现（对照实验，同字节数 2 MiB）：
  ```bash
  # LF 版：1,048,576 行 → 被拒
  printf 'a\n%.0s' $(seq 1048576) > lf.txt   # 等价 python 生成
  # 返回：{"ok":false,"state_known":true,"error":"移动源 …/lf.txt 为 1048576 行，超过 100000 行读入上限"}
  # lone-CR 版：同字节数 → 通过并执行
  python3 -c "open('cr.txt','w').write('a\r'*1048576)"
  printf '*** Begin Patch\n*** Update File: cr.txt\n*** Move to: dst.txt\n@@\n+z\n*** End Patch\n' | terminal-chat --internal-apply
  # 实测返回：{"ok":true,"changed_files":[add dst, delete cr.txt],"state_known":true}
  # /usr/bin/time 等价测量：耗时 0.53s、峰值 RSS 165MiB（真实展开 1,048,576 行）
  ```
- 建议（低成本）：行数门与 `SourceFile::parse` 统一行定义——门检查改用与解析一致的行计数（把孤立 `\r` 计入），或解析后按实际行数复查上限再进入替换计算。
- 涉及位置：`apply_patch.rs` `logical_lines()`（读入门，普通 Update 与 Move 共用）。

### F-2【低】procfs 伪文件经 Move 物化为常规文件（env/maps 复制原语）；沙箱面已实测收口

- 后果：`/proc/self/environ`、`/proc/self/maps` 是 `S_IFREG` 且 `st_size=0`，通过 `fd_is_regular` 判型。纯移动**无需任何内容知识**：先排他创建目标并完整写入，随后删源（procfs 上 unlink 必败 EPERM），结果＝内容已落盘并进入展示面（`PatchChange::Add{content}`），`ok=false`、`state_known=false`、清单含目标 `add`。实测：
  - 非沙箱：`FAKE_SECRET_TOKEN=FAKE-SECRET-12345` 下 `*** Update File: /proc/self/environ / *** Move to: env-copy.txt` → env-copy.txt 3931 B 含全部环境变量（假密钥实测在列）；`/proc/self/maps` 同样物化（泄漏 ASLR 布局/库路径）。
  - 沙箱（TCHAT_SANDBOX=1，真实 bwrap）：子进程环境为白名单（实测仅 `HOME/PATH/TERM/PWD`），假密钥在文件与展示面均为 0 命中——**已收口**。
  - 对照：普通 Update 无法复现——environ 无换行（NUL 分隔）连匹配都不可能；即使匹配，写路径也会失败。Move 是唯一物化路径。
- 定级依据：非沙箱模式本就是信任面（模型可任意 exec，`printenv` 等价能力已存在，027 环境防线只在沙箱分支声明）；沙箱下已收口。故为纵深防御缺口而非可利用漏洞。
- 建议（低成本）：Move 源判型时拒绝 procfs/sysfs 类源（如 `fstat` 后校验设备号，或对 `/proc/`、`/sys/` 前缀显式拒绝"非常规可删除文件"），消除"复制不可删除对象"这一不对称能力。
- 复现（非沙箱）：
  ```bash
  printf '*** Begin Patch\n*** Update File: /proc/self/environ\n*** Move to: env-copy.txt\n*** End Patch\n' \
    | terminal-chat --internal-apply
  # 返回：{"ok":false,"changed_files":[{"path":"…/env-copy.txt","action":"add"}],"state_known":false,
  #        "error":"删除移动源失败（源 /proc/self/environ，目标 …/env-copy.txt）：Operation not permitted (os error 1)"}
  # env-copy.txt 已存在且含完整环境
  ```

### F-3【低/文档】非沙箱模式无任何路径约束——`../`、绝对路径、目录符号链接均可越出工作区

- 后果：`Move to: ../outside.txt`、`Move to: /tmp/x`、工作区内目录链接 `lnk→/tmp/real` 均成功把文件"移"出工作区（实测均 ok=true）。规格 §1.2 明示"绝对路径原样使用""权限由执行身份及执行环境控制"——属设计内信任面，安全完全依赖 `TCHAT_SANDBOX=1`（033）。裸跑（未注入开关的部署）默认**无沙箱**；共享 dev 已默认注入。
- 沙箱对照实测（真实 bwrap）：`/workspace/...`、`/etc/...` 目标 → `Read-only file system (os error 30)` 拒绝、零落盘；`lnk→/etc` 目录链接 → 同样 EROFS；`../` 到 `/tmp` 成功（/tmp 属设计内可写根，落点为真实 /tmp，策略明文允许）。
- 建议：无需改代码；在用户文档/部署说明明示"未设置 TCHAT_SANDBOX 时 apply_patch 无路径边界"。

### F-4【低/信息】路径拼写边角的静默接受与展示不归一

- `*** Move to:   spaced.txt`（冒号后多一个空格）静默创建首部含空格的文件名 `"  spaced.txt"`（实测 ok=true）——与 004 既有路径空白处理一致，易造成"文件不见了"的观感混淆。
- 成功结果/展示路径保留原拼写不归一：`"/tmp/sec029/work/../sb-escape.txt"`、`"/tmp/sec029/work/sub/../a.txt"`（实测原样出现）。仅展示混淆，无安全影响。
- 建议：可选地在 `check_path` 去除目标首部空白；无需改路径归一（会破坏"原样使用"契约）。

### F-5【信息】第四级等价类会命中"人眼不同"的行（规格定义行为，提示误改风险）

- `token=‘abc’`（弯引号）可被 ASCII 补丁 `-token='abc'` 命中并替换（实测 ok=true）。这是 §1.3 明文等价类。误改防护的真实防线是**级间优先级**：实测早处弯引号候选、晚处精确候选同存时，精确候选胜出（较严格级先扫描完整范围）；`@@` start 推进后早处候选不越界；未映射相似字形全部拒绝（见下）。
- 用户可见后果：作者以为在改"长得一样"的 ASCII 行，实际可能改了弯引号行（反之亦然）——两者映射后同文。属功能语义，建议在工具描述/文档保持现有"映射仅定位"的明示。

### F-6【信息】"删除"不等于数据不可恢复：硬链接别名保留旧内容

- `ln src alias` 后移动 src→dst：alias 仍保有原内容（实测）。§1.4.7 已声明这是"新建＋删除"语义、不承诺 inode。提示依赖 Delete 做数据销毁的预期不成立。

### F-7【信息】删源复核与 unlink 之间的残余竞态窗口（规格已声明不承诺）

- 100 轮外部竞态（目标出现即原子换源）未命中复核 open→unlink 之间的微秒级窗口；换源落在复核前时被稳定拦截：`"移动源对象在删除前被更换，已保留目标 add、不删除替换对象"`（实测抽样）。§1.4.6 明示不承诺与外部并发的事务隔离。无需修复。

### F-8【信息】SIGXFSZ 默认处置下写入超限＝进程信号终止（153），调用方看不到 JSON

- `ulimit -f` 且不忽略 SIGXFSZ 时：进程被终止，dst 残片 ≤1 KiB、源完好（实测）。沙箱父进程侧会以 `"沙箱执行失败（exit …）"`、`state_known=false` 呈现（`apply_patch_sandboxed` 代码路径），语义安全。可选增强：`run_internal_apply` 阻塞/忽略 SIGXFSZ 让 EFBIG 走常规失败 JSON。非必须。

---

## 已验证有效的防线（攻击被拦截清单）

| # | 攻击 | 实测结果（真实返回摘要） |
|---|---|---|
| A1 | 源目标词法等价（`./a`、`sub/../a`、绝对同路径） | `移动源与目标为同一路径，按目标冲突拒绝`，零操作 |
| A4/E4/E4b/E9b | 越界写 `/etc`、`/workspace`（沙箱） | `Read-only file system (os error 30)`，零落盘，state_known=false |
| B1-B3 | 源=符号链接/悬空/循环 | `移动源是符号链接（含悬空），拒绝移动`（O_NOFOLLOW→ELOOP） |
| B4 | 源=FIFO | 65 ms 快速拒绝，无阻塞：`移动源不是常规文件` |
| B5/B6 | 源=/dev/null、目录 | 同上拒绝 |
| B7-B9 | 目标=硬链接/符号链接/悬空链接（预存在） | `移动目标已存在，禁止覆盖`，源目标均完好 |
| C1 | 目标抢占竞态（4 MiB 源放大窗口，30 轮） | 30/30 `create_new` 排他拒绝；源完好；攻击者文件未被覆盖 |
| C2 | 删源窗口源替换 | 复核拦截：`移动源对象在删除前被更换，已保留目标 add、不删除替换对象` |
| D1 | 补丁 1 MiB+1 | `internal-apply：补丁超过 1048576 字节上限`（exit 2） |
| D2 | 源 4 MiB+1 | `…超过 4194304 字节（4 MiB）读入上限`，源不动 |
| D3 | 源 100001 行 | `…超过 100000 行读入上限`，源不动 |
| D5 | 移动并修改膨胀至 4.4 MiB | 写前拒绝 `生成内容 4597762 字节超过…上限`，未建目标未删源 |
| E1/E2 | 沙箱内移动被遮蔽 `.env`（纯移动/带修改） | 打开源即 `Permission denied (os error 13)`，正文不可得，零泄漏 |
| E5 | 沙箱内移动 `/proc/self/environ` | 内容仅 HOME/PATH/TERM/PWD；FAKE-SECRET 在文件与展示面 0 命中 |
| E6 | `.env` 硬链接别名（nlink=2） | `沙箱不可用：受保护文件 …存在 2 个硬链接…拒绝启动沙箱工具（不回退普通模式）`（fail closed） |
| E7/E7b | bwrap 不可用 | `未在 PATH 中找到 bubblewrap…`，零执行（文件原样） |
| E8 | 沙箱内普通移动正例 | 成功（防线不误伤合法操作） |
| F1 | 28 码点双向匹配（56 用例） | 56/56 按表匹配，未扩大 |
| F2 | 严格级 vs 四级候选竞争 | 精确候选胜出（整段范围先扫严格级） |
| F4 | 未映射字形 9 例（全角/组合/U+2000/2001/200B/FF0D/2043/FE63/①） | 全部不匹配，文件零改动 |
| F5/F8 | 映射不写回 | 上下文弯引号字节 `e2 80 9c/9d` 原样保留（od 佐证），仅替换差量 |
| F6/F7 | start 推进不越界、EOF 锚定＋四级 | 定位正确，早处候选不越 start |
| G1 | 删源失败（源父目录 555） | `ok=false`、清单仅目标 `add`、`state_known=false`、错误含源目标双路径与阶段；dst 完整、src 完好 |
| G2 | 半写（RLIMIT_FSIZE→EFBIG） | `写入移动目标失败…File too large`、不虚报 add、state_known=false；dst 残片、src 完好 |
| G3 | 重复提交同一移动 | 第二次 `移动目标已存在`，目标字节不变 |
| G4 | 合法前序块＋后续 Move 非法 | 整体不执行（完整解析先行），空清单、state_known=true |
| G6 | 链式 A→B→C 中途失败 | 不回滚，清单按序累积 4 条（add b, delete a, add c, delete b） |
| H1-H4/H7 | 前缀空格伪装/修改行后/EOF 后/End Patch 后/重复/Delete 块内 Move | 全部精确拒绝（含行号），零文件操作 |
| H5/H8 | CRLF 补丁＋混合行尾源、中文空格路径、无末尾换行 | 逐字节保留（md5 相同/od 佐证） |
| H6 | 目标为 `.`/`..` | `移动目标已存在，禁止覆盖` |

## 修复优先级建议

1. F-1（行数门与解析行定义统一）——低成本、直接违背规格 A9 边界。
2. F-2（Move 源拒绝 procfs/sysfs）——低成本纵深防御，消除唯一物化路径。
3. F-3/F-4/F-5——文档与可选体验项，无代码必须项。

## 证据与可复现产物

- 攻击脚本与原始输出：`/tmp/sec029/`（lib.sh、a_path.sh、b_link.sh、c_race.sh、d_resource.sh、f_unicode.sh、g_h.sh、e_sandbox.sh、e2.sh、out/）
- 沙箱探针（走生产入口 `apply_patch_sandboxed_with_exe`）：`/tmp/sec029/probe/`（依赖 `/workspace/src/app`，独立 `--target-dir`，未污染共享 target）
- 本报告不落原始日志；关键请求/返回已逐条内嵌上文。
