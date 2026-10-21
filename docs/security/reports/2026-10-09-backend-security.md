# 后端安全攻防测试报告（2026-10-09）

> 范围：terminal-chat 后端真实接口——库级 JSON 工具边界（`exec_command`/`apply_patch`）、出站 LLM 客户端与授权门（`api::chat`/`origin_of`/`ensure_origin_authorized`/`validate_base_url`）、配置与凭证（`.env`/manual/凭证解析）、显示边界（`sanitize_for_terminal`）、CLI 管道入口。
> 方法：先攻击者视角代码审查，再用可运行脚本对真实入口实弹攻击（本地 mock 记录请求与凭证头；隔离临时目录；不触网、不碰真实凭证）。攻击脚本与原始输出位于 /tmp/attack/（一次性，报告自包含复现步骤）。
> 环境非 root（uid 1000）、Linux。既有回归 149 项全绿为基线；本次为增量攻防。

## 〇、概念映射（如实声明）

本工程无 Web 多租户后端。8 类攻击映射如下，不存在的一律 N/A，不编造概念：

| 类别 | 本工程对应面 | 结论 |
|---|---|---|
| 1 数据越权 | 无订单/用户 ID/多租户数据。最近似：工具读写工作区外文件 | 规格（003/004）明确允许相对/绝对任意路径，权限由执行环境（OS 权限）控制；实测 `../`、绝对路径、emoji 路径可达且受 OS 权限约束（只读目录/只读文件拒绝，既有回归覆盖并复验）——按设计，非缺陷 |
| 2 功能越权 | 无角色/管理员接口。最近似：凭证外发授权门 | 见电池 B：全部伪造被拒 |
| 3 身份认证 | 无登录/token。最近似：出站凭证授权（origin+指纹双匹配） | 见电池 B：userinfo/大小写/换 scheme/重定向伪造全部失败 |
| 4 偷改字段提权 | JSON 工具入参与配置文件的未知/保留字段 | 既有回归覆盖（未知参数拒绝、deny_unknown_fields、request_params 保留字段拒绝）；本轮加测 JSON 重复键（取后者，无放大） |
| 5 恶意输入与注入 | 无 SQL（无数据库）；命令执行是 exec_command 的功能本体；补丁/配置/URL/键值为注入面 | 见电池 A/D：边界输入不 panic、不放大；显示注入被过滤 |
| 6 绕过前端限制 | REPL 即"前端"，库函数为直连通道 | 集成测试即"绕过 REPL 直打接口"的既成事实（149 项）；管道模式无前端校验依赖 |
| 7 敏感信息泄露 | 错误消息/Debug/审计/回显 | 既有覆盖（Key/指纹不泄露、Debug 脱敏）；本轮实测恶意回复与 hostile URL 的终端注入均被滤净 |
| 8 资源滥用与防刷 | 无登录/短信/下单；限流属远端服务责任。本地面：超大补丁/大文件/特殊文件/超长输入 | 见发现 F2（FIFO 无限阻塞）与电池 E（大输入无放大） |

另：工作区 `db/wiki.db` 与后端无关（源码零引用，系 wiki 工具产物），范围外。

## 一、发现清单

### F1【已修复 2026-10-09】`.env` 可重定位信任根，配合预置授权记录实现静默凭证外发
- **后果**：受害者克隆含恶意 `.env` 的仓库并在其中运行 CLI 后，凭证可无提示发往攻击者服务器（实测成功外发）；亦可重定向配置目录造成钓鱼混淆。缓解因素：授权按 (origin, sha256 截断 16-hex 指纹) 双匹配，攻击者需已知凭证指纹（64bit 不可暴力；但曾见过 key 即可算）。
- **根因**：`catalog::load_dotenv_from`（src/catalog.rs:193）接受任意 `[A-Za-z0-9_]+` 变量名，包括测试/部署钩子 `TCHAT_USER_DIR`、`TCHAT_CONFIG_DIR`；`.env` 是未声明的信任根，启动审计不检查其内容。
- **复现**（实测，/tmp/attack/envdemo）：仓库 .env 写 `TCHAT_USER_DIR=/tmp/attack/envdemo/attacker` 与 `DEEPSEEK_API_KEY=victim-secret-key`；攻击者目录预置 `authorized-origins.json`＝`{"grants":[{"origin":"http://127.0.0.1:<mock端口>","cred_fp":"<fingerprint(victim-secret-key)>","added_at":0}]}`；`TCHAT_CONFIG_DIR=... terminal-chat < 你好\nexit`。**实测结果：mock 收到 `POST /chat/completions` 且 `Authorization: Bearer victim-secret-key`；stdout 出现 `Agent：`（对话成功），全程无授权询问；退出码 0。** 对照组（干净 user 目录）：零请求、错误"未授权发送到 …非交互模式…已拒绝发送"，退出码 0。
- **修复建议**：`load_dotenv_from` 跳过 `TCHAT_` 前缀变量（或启动审计对 .env 设置 `TCHAT_*` 显著告警）；在 AGENTS.md/README 声明 .env 为信任根。

### F2【已修复 2026-10-09】`apply_patch` 对 FIFO 等非常规文件 Update 无限阻塞，且无取消机制
- **后果**：补丁指向命名管道时，`fs::read` 阻塞等待写端，工具调用永久挂起（**实测 `timeout 6` 杀死，退出码 124**，全程无输出）；`exec_command` 有超时与取消标志，`apply_patch` 两者皆无，宿主只能杀进程。攻击面：任何能控制补丁内容或工作区文件布局的一方（恶意模型输出、被投毒仓库中的 FIFO）。
- **根因**：`apply_hunk` Update 分支（src/apply_patch.rs）对目标仅做存在性与 UTF-8 检查，未校验 `symlink_metadata().file_type().is_file()`；读/写无超时。
- **附带（低，同根因）**：`Update File: /dev/null` 实测返回 `ok=true` 并把 `/dev/null` 计入 changed_files——字符设备写入静默丢弃，变更清单语义失真但无破坏。
- **修复建议**：Update/Add/Delete 前校验目标是常规文件（FIFO/设备/套接字拒绝）；如需更强保障，为 apply_patch 引入超时或取消标志（对齐 exec_command）。

### F3【信息，按设计确认】路径无限制、无放大、边界输入健壮
实测通过项（攻击者视角"打了没打穿"的证据）：
- `../` 穿越、绝对路径、中文/emoji 路径：可达但受 OS 权限约束（规格允许；权限即边界）。
- JSON 重复 `patch` 键：serde_json 取后者，与常规解析语义一致，无放大。
- 1.4MB 巨型补丁：2ms 完成，落盘 1,400,000 字节＝输入规模（无放大）。
- 补丁含 NUL/控制字符：作为 UTF-8 数据落盘（内容即数据，不被执行）。
- 仅 CR 分行补丁：解析拒绝（第 1 行报错）。
- `exec_command` cmd 含 NUL：优雅报错"进程启动失败：nul byte found…"；`timeout_ms=u64::MAX`：无 panic 正常执行。
- 200k 行文件 × 500 @@ 块：51ms（实测无性能恶化）。

## 二、通过项（实弹攻击未突破）

**授权门伪造（电池 B，双 mock：A＝已授权、B＝攻击者，判据＝B 收到 0 个请求）**：
| 攻击 | base_url 构造 | 实测结果 |
|---|---|---|
| userinfo 欺骗 | `http://127.0.0.1:{A端口}@127.0.0.1:{B端口}` | origin＝B → 门拒绝，B 零请求 |
| localhost 伪装 | `http://localhost@127.0.0.1:{B}` | origin＝B → 拒绝 |
| 大小写混淆 | `HTTP://127.0.0.1:{A}` | 规范化为 A，合法放行（良性） |
| 换 scheme 混淆 | `https://127.0.0.1:{B}` | origin 不同（https≠http）→ 拒绝 |
| 路径尾随垃圾 | `http://…{A}/x%1b` | origin 只绑 host:port → 放行至 A，无混淆 |
| 302 重定向投递 | 授权 A，A 返回 302→B | "拒绝跟随重定向"，B 零请求 |

**显示注入与 CLI 管道（电池 D）**：mock 恶意回复内嵌 OSC 改标题/剪贴板/CSI 清屏/终端参数序列 → stdout ESC 字节数＝0；配置错误消息含转义序列的 hostile URL → stdout ESC 字节数＝0；8MB 单行管道输入 → 干净处理退出码 0；非法 UTF-8 stdin → 无 panic、`再见!`、退出码 0。

## 三、修复优先级
1. F2：非常规文件校验（一行 `is_file()` 门禁即可封 FIFO 挂起与 /dev/null 失真）。
2. F1：`.env` 跳过 `TCHAT_*` 或审计告警 ＋ 文档声明信任根。
均未修改业务实现（本轮为测试轮）；建议下一轮按 tdd 修复并补回归。

## 五、修复记录（2026-10-09 同日）

**F1 修复**（src/catalog.rs）：新增 `TRUST_CONTROL_VARS`＝[TCHAT_CONFIG_DIR, TCHAT_USER_DIR, TCHAT_PUBLIC_CATALOG_URL, HOME, APPDATA]（全库运行时 env 读取点审计所得，非前缀一刀切——普通凭证变量仍可从 .env 加载），`load_dotenv_from` 对名单做 `eq_ignore_ascii_case` 跳过（大小写不敏感防 Windows 绕过，独立 Review 必修项）。
- RED→GREEN：单测 `dotenv_skips_trust_control_vars`（含小写变体）修复前断言失败、修复后通过。
- E2E 复验（新编译候选，真实 CLI＋本地 mock；受害者凭证 `sk-victim-real-9f3a` 在可信进程环境，攻击者文件只含 .env 目录劫持与有效指纹授权记录）：**修复后 mock 收到 0 个请求**，拒绝提示指向受害者家目录 `…/victim-home/.config/terminal-chat/authorized-origins.json`（授权根未重定位）。
- 三正例：调用者显式 `TCHAT_USER_DIR`＋合法预授权 → 请求携带 `Bearer sk-victim-real-9f3a`；.env 普通凭证变量 `NORMAL_DOTENV_KEY` → 请求携带 `Bearer dotenv-loaded-key-77`；`TCHAT_CONFIG_DIR` 显式设置全程生效。

**F2 修复**（src/apply_patch.rs）：Update 分支在 `fs::read` 前以 `fs::metadata`（跟随链接、stat 不阻塞、与 read/write 同语义）判型，非 `is_file()` 即校验型拒绝（state_known=true）；Add/Delete/链接语义不变（a1–a6 全绿）；判型与读之间残留竞态与存在性检查同级（注释载明）。
- RED→GREEN：新测试 `f2_update_rejects_non_regular_targets`（子进程＋15s 看门狗自保）修复前两 FIFO 例看门狗超时（挂起）、/dev/null 例假成功断言失败；修复后 0.22s 内三例全部明确报错（"不是常规文件"）、FIFO 本体原样、常规文件对照正常。
- harness 复验（新 rlib 重编）：`timeout 8 ./harness fifo` 快速返回（此前退出码 124 挂起）；`Update /dev/null` ok=false 空清单（此前 ok=true 假成功）；路径穿越/授权门电池/200k 行性能无回退。

**回归**：`cargo check` 0 警告；`cargo test --workspace --no-fail-fast` 退出码 0，**151 通过 / 0 失败 / 0 跳过**（基线 149＋新增 2 用例）。F3、Q1–Q4 及其余边界分类不变（仅 F2 使 F3 中"`/dev/null` 计入清单"一条旧观察失效，以本节为准）。

### F2 收口（同日第二轮：判型与读写的对象替换窗口）

上一轮 F2 的 `fs::metadata` 判型与 `fs::read`/`fs::write` 都按路径解析，之间存在替换窗口：判型通过后把路径换成 FIFO，写入按路径重开会永久阻塞。本轮收口（src/apply_patch.rs）：
- 读入：`open(O_RDONLY|O_NONBLOCK|O_CLOEXEC)` 先打开（FIFO/设备打开本身不阻塞），对**打开的 fd** `fstat` 判 S_IFREG，`read_to_end` 走同一 fd——判型绑定实际读写的 inode。
- 写入：经 `/proc/self/fd/<读fd>` 以 `O_WRONLY|O_TRUNC` 重开**同一 inode**（O_TRUNC 只作用于已判型对象），重开后比对 `(st_dev, st_ino)` 与读句柄一致且仍为常规文件（纵深防御）；/proc 缺失时明确报错不回退路径写。
- 确定性 RED：新测试 `f2_swap_to_fifo_in_update_window_binds_to_validated_inode`——6M 行×6 万块补丁使"读入→计算"约 2~3 秒（受控窗口），窗口内（1s 时）把目标改名换入 FIFO 或以原子 rename 把链接替换为 FIFO（无中间状态）；修复前两连看门狗超时（15.6s 挂起），修复后 5s 内通过：写入绑定原 inode（改名后的原文件/real.txt 内容已更新）、FIFO 占位原样未动、清单正确，且修复后终态与替换落在窗口内还是完成后无关（确定性成立）。
- 复验（新编译候选）：静态 FIFO/链接到 FIFO//dev/null 即时拒绝不变；A6 路径行为、B7 重定向、E1 性能无回退；只读文件 0444 经 /proc/fd 重开仍 EACCES（无提权，Review 实测）。
- 独立 Review：对象绑定/打开不阻塞/权限语义/破坏性/EFBIG 契约全部确认；必修项（测试变体二时点失控与 remove/symlink 间隙竞态）已改为大文件受控窗口＋原子 rename；两项纵深防御（inode 比对、/proc 文案）已采纳。
- 验证边界（如实保留）：不做事务/并发一致性承诺——路径被替换时变更落在原 inode 上，changed_files 报告补丁路径（Unix fd 语义）；判型前路径解析本身仍可与外部并发（与存在性检查同级）。

### F2 最终验收（同日第三轮：回归测试确定性收口）

窗口替换测试由"大文件＋计时窗口"升级为**真实同步屏障**：`src/apply_patch.rs` 新增 `#[cfg(test)] f2_seam`（生产构建零符号，env 仅测试代码读取，不构成公开参数）——Update 完成打开＋判型＋读入＋计算后、写入前写 `reached` 标记并等待 `release`。单元测试 `f2_swap_barrier_binds_write_to_validated_inode`（子进程 `--exact` 重入＋15s 看门狗）：控制端确认 `reached` 后在**确定性的写前时点**完成 FIFO 替换（变体一改名占位；变体二原子 rename 替换链接本体）再放行写入。断言：ok=true、state_known=true、changed_files 精确、原 inode 内容逐字节正确（`R1\n…R5\n`）、FIFO 仍为 FIFO 未被读写。0.07s 完成，5 连跑稳定（Review 复测）。
- 专用副本（显式 `--target-dir /tmp/f2mut-target`）变异证明：写回按路径重开（`fs::write`，接缝保留）→ 屏障测试**看门狗红 15.01s**（写入阻塞在 FIFO——确定性触发，无计时依赖）；恢复后绿、副本源码与工作区字节一致。Review 在独立副本复证同样结果。
- 平台范围（如实声明）：`/proc/self/fd` 重开与 `O_NONBLOCK` 为 Linux 语义，与 AGENTS.md 平台约定一致；/proc 缺失 fail-closed。本次全部验证在当前 Linux 容器完成，未验证其他平台。
- Review：同步顺序（reached 语义、swap→release→写严格有序、屏障前退出检出）、业务断言充分性（"仍为 FIFO"＋看门狗＋ok 断言组合）、接缝合规（生产二进制零符号）、覆盖无丢失（严格覆盖旧计时测试两变体并增强）全部确认；无必修项，可选增强（变体二 state_known 断言）已采纳。旧计时窗口测试已移除（被取代）。

## 四、材料
- 攻击 harness：/tmp/attack/harness.rs（电池 A/B/E、fifo 子模式）；mock：/tmp/attack/mock.py；.env 链：/tmp/attack/envdemo/（含实验/对照组完整原始输出）。
- 既有相关回归：tests/llm_flow.rs（授权门/泄露扫描/重定向）、tests/apply_patch.rs（权限门/链接/EFBIG）、src 内嵌单元（origin/URL/凭证规则）；2026-10-05 安全修复报告为本报告前置。
