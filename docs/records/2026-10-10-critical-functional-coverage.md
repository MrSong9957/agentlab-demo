# 高风险功能覆盖核验与加固记录（2026-10-10）

> 输入：`docs/testing/critical-functional-test-cases.md`（**111 条**唯一编号索引；首轮日志误计
> 112/AUTH 13，收尾轮已按唯一编号重计修正为 AUTH 12、总 111）。行为真源：本地最终 001–006
> 规格、2026-10-05/09 安全修复、AGENTS.md。全程假 Key、本地 mock、隔离临时目录、非 root（uid 1000）。
> 首轮基线：`cargo check` 0 警告（0）；`cargo test --workspace --no-fail-fast` 190/0/0（0）。
> 首轮终态：209/0/0（0）。**收尾轮终态：214 通过 / 0 失败 / 0 跳过**（85＋32＋9＋14＋51＋9＋14），
> `cargo check` 0 警告（退出码 0）。
> 统计口径（分开，不混用）：**用例状态**按 111 个唯一编号；**新增测试函数**两轮共 24 个
> （首轮 19＋收尾轮 5）。

## 一、缺陷修复（RED→GREEN，最小修复）

| 缺陷 | RED 证据 | 修复 | GREEN |
|---|---|---|---|
| **SECRET-E02（回显）**：服务端在 error.message 回显完整凭证时，错误消息把 Key 原样打印（300 截断可成为唯一"防线"） | `secret_e02_server_echo_of_key_is_redacted`（回显在开头/300内/300外三位置）与 `secret_e02_real_cli_output_never_echoes_key`（真实 CLI stdout/stderr 扫描）均失败：错误文本含 Key | `api.rs` 新增 `redact_credential`，在 `chat`/`chat_with_body`/`list_models` 的发送与解析错误边界抹除当前凭证值；既有错误文案断言不受影响 | 两测通过；llm_flow 46/46 |
| **SECRET-E02（顺序，收尾轮）**：处理链为截断在前、脱敏在后——完整 Key 横跨 300 字符边界时被切断，整串替换匹配不到残留片段（实测错误尾部残留 `test-key-A` 10 字符） | `secret_e02_straddle_boundary_no_fragment`（Key 起点 290、预选 10 字符独特片段检测）；首轮"RED"实为测试算术错误（Key 15 字符误算 16），已按严格顺序重做：临时还原旧顺序复现真实行为 RED→恢复修复 GREEN | `describe_status_error` 增加 `secret` 参数并**先脱敏后截断**；`send` 透传（list_models 传 Key、公共目录传 None） | 三测（含横跨）全过 |
| **TERM-B03**：字符集选择 nF 序列（`ESC ( B`，ECMA-48 中间字节＋终字节）被当两字符转义处理，残留终字节为文字（`"a\x1b(Bb"→"aBb"`）——违反 display 模块自述"完整解析并剥离 ECMA-48 转义序列" | `charset_selection_escapes_removed` 失败：`left "aBb" right "ab"` | `display.rs::skip_escape` 拆分中间字节（0x20–0x2F 可重复）与终字节（0x30–0x7E）分支，标准精确 | display 9/9；e2e 9/9 无回归 |

契约同步：AGENTS.md §7 行为清单已更新（nF 序列完整剥离＋服务端回显凭证必须脱敏）。

## 二、敏感性证明

- SECRET-E02 两测：缺陷 RED 即证据（修复前确定性失败）。
- **变异 A**（隔离副本将 Add 的 `create_new` 改覆盖写）：`patch_b05` 失败（检测到覆盖/混合），对照 `a1_update` 通过。
- **变异 B**（副本忽略 `grant_origin` 落盘失败）：`auth_e02` 失败（同意后仍外发），对照 `s2a_pty` 通过。
- 副本已删除（`--target-dir` 专用，未触碰工作树）。

## 三、新增测试（19 个）

AUTH-N04/E02（PTY 真实入口）；NET-B01/B03；TRUST-B05/B06；EXEC-B04/B05/B07/E03/P01（调查）；
PATCH-B05；LOOP-B05；SECRET-E02×2；TERM-N02/B03/E01；display 截断边界。
测试侧修正（脚本问题非产品缺陷，如实记录）：auth_n04 等待顺序（refresh 汇总在凭证询问之后）
与所选模型（刷新替换名单不切模型）；exec_b04 两次收紧断言（跨流时序无契约、200 码点精确
前缀可落行中——均按清单自身规定修正，10 连跑稳定）；loop_b05/display 追加位置移入模块内。

## 四、受控调查与决策点（未修复、未固化）

1. **AUTH-B05 并发授权**：两线程并发首次授权不同 origin——**两个调用都返回成功，但文件只保留
   1 条**（读-改-写后写覆盖前写；文件可解析、无未同意组合）。是否承诺"都保留"需决策；
   现状测试 `auth_b05_concurrent_grants_investigation` 只断言安全不变量。
2. **EXEC-P01 超时上限**：`timeout_ms=u64::MAX` 被接受——500ms 观察窗内命令仍运行，随后经
   模块取消入口回收进程组（既有 003 契约内）。有限上限值/超限处理未定，未自行加常量。
3. **EXEC-P02 REPL 取消**：聊天入口未接取消（`turn.rs` 调 `exec_command` 时 `cancel=None`）；
   触发方式与恢复时间未定，只记录。
4. NET-B05 尾随空格：现状为 URL 解析拒绝（安全），"拒绝 vs 规范化"未成文。
5. SECRET-E03 转义嵌入：完整原串三位置已覆盖；转义/多字段嵌入未展开（文档本身不要求屏蔽短子串）。

## 五、覆盖矩阵（111 个唯一编号；状态细分：已有覆盖 / 新增通过 / 部分覆盖〔注明人工记录·源码推理·未展开〕/ 待决策 / 环境受限）

**1 AUTH（12）**：N01 已有（e2e s2a：grant 字段/0600/added_at>0）；N02 部分覆盖（同会话免重复询问自动＋**跨进程复用为人工记录** logs/2026-10-09 手测#6）；N03 已有（s2b：零请求/不落盘/含 origin）；N04 **新增通过**（auth_n04 PTY：refresh 双询问各恰一次、Bearer 列表请求、双 grant 含 catalog 标记）；B01 已有（origin_of_normalizes：默认端口归一/路径不参与）；B02 已有（semantics 换地址不放行＋C3）；B03 已有（C3 换 Key 拒绝＋semantics 复用语义）；B04 已有（semantics 初始空表＋C2）；B05 **待决策**（auth_b05 调查：双方成功但仅保留 1 条，见四.1）；E01 **新增通过**（auth_e01 四变体：空文件/JSON数组/根数组/字段类型错→非交互零请求且记录原样）；E02 **新增通过**（auth_e02 目录占位写故障→零请求、不外发）；E03 已有（C2 非交互；不可用路径落同 E02 写失败路径）。

**2 TRUST（13）**：N01 已有（s3b 正例一：.env 值＝请求头 Bearer）；N02 已有（s3b 正例二显式根）；N03 已有（confined 根内取值＋refresh 用例 Bearer）；B01 已有（s3a 未预设攻击：双端点零请求、授权根未重定位）；B02 已有（dotenv 不覆盖＋s3b）；B03 已有（dotenv_skips 含小写变体）；B04 已有（穿越/链接逃逸/目录外拒绝且不泄露内容）；B05 **新增通过**（trust_b05 链接根内可用、经链接逃逸拒）；B06 **新增通过**（trust_b06 BOM 跳过＋CRLF 解析＋控制变量两形态不生效；引号/注释/重复键已有）；E01 已有（s3a 完整链）；E02 已有（confined 三类逃逸拒绝）；E03 已有（credential_file_error_branches 五分支）；E04 已有（BAD LINE 跳过、后续行正常）。

**3 NET（12）**：N01 **环境受限**（无可信本地 TLS 夹具；按文档拆分：URL 准入 validate_base_url https＋共享发送路径 http 回环；TLS E2E 受限如实记录，未关证书校验凑通过）；N02 已有（chat_sends＋C1）；B01 **新增通过**（net_b01：300/301/302/303/307/308/399 全拒、目标零请求）；B02 **新增通过**（net_b02：同 origin/无 Location/Location 非法均无第二跳）；B03 **新增通过**（net_b03：三写法豁免、127.0.0.2 拒）；B04 已有（slash_and_v1_paths）；B05 部分覆盖（默认端口自动✓；尾随空格＝解析拒绝属**源码推理**，"拒绝 vs 规范化"未成文）；B06 **新增通过**（net_b06：L−1/L/L+1 独立字节核对、前两者成功且内容完整、L+1 明确 10 MiB 超限）；E01 已有（route_rejects＋production_catalog）；E02 已有（stale_switch 开关不旁路）；E03 部分覆盖（userinfo 伪装为安全报告电池 B **一次性人工攻防记录**；origin 按 host 代码已核）；E04 已有（disconnect/timeout/拒绝连接分类＋无重试）。

**4 EXEC（17）**：N01/N02/N03/N04 已有（workdir 相对路径、双流＋退出码 7、pwd、cancel_flag）；B01/B02/B03/B06/B08 已有（400ms 超时、码点截断、N/N+1、非法 UTF-8→U+FFFD、s6 组终止终态）；B04 **新增通过**（exec_b04 双流 MB 级排空收尾，断言按"跨流时序无契约＋精确码点前缀"两次修正后 10 连跑稳定）；B05 **新增通过**（exec_b05 8192 边界无损重组）；B07 **新增通过**（exec_b07 并发组隔离：A 超时期间 B 五连发全成）；E01/E02 已有（invalid_inputs 九类/args_reject 含 null vs 缺省）；E03 **新增通过**（exec_e03 NUL 启动失败零副作用＋补齐）；P01 **待决策**（exec_p01 调查：u64::MAX 窗口内仍运行、取消回收；上限值未定）；P02 **待决策**（REPL 取消未接通）。

**5 PATCH（20）**：N01–N05 已有（cycle 字节/d1 级联/a1/a2 链接）；B01–B04 已有（b6/b7、行尾、b1–b5/b8 容差）；B05 **新增通过**（patch_b05 并发 Add 恰一胜者、内容完整属胜者）；B06 部分覆盖〔**源码推理＋结果锚定，非受控竞态测试**〕——源码：`open_validated_regular` 以 `O_RDONLY|O_NONBLOCK|O_CLOEXEC` 打开（FIFO 打开不阻塞），`fd_is_regular` fstat 绑定**打开的 fd**，写入经 `write_via_validated_fd` 走 `/proc/self/fd/<读fd>` 同 inode 并二次核验 (dev,ino)＋is_file——打开前替换＝打开到被换对象→判型拒绝，阻塞窗口结构性消除；测试锚：静态 FIFO 有界拒绝（f2，看门狗子进程）＋打开后替换绑定（f2_swap_barrier）。受控"打开前"竞态需 open 前接缝，未新增；B07 已有（屏障受控竞态：写绑定原 inode、FIFO 占位未动）；B08 部分覆盖（大输入为安全报告 F3 **一次性记录**，本轮按文档不另堆压测）；E01–E07 已有（解析失败零写入/已存在/权限/f2 特殊文件/d2 保留前项/EFBIG 半写 d6+second_file/UTF-8）。

**6 LOOP（17）**：N01–N06 已有（t1–t4/t6/t11、chat_with_body 普通参数＋核心字段）；B01–B04/B06 已有（t1/t9、t10 content 保真、t14 第 8 次回复、t13 计数重置、多轮完整解析）；B05 **新增通过**（loop_b05 十调用批：十个真实文件副作用、id 一一对应、恰 2 次请求）；E01–E05 已有（t9＋单测、t12＋parse 单测、t5＋单测、t8、守卫两测）。

**7 SECRET（10）**：N01 已有（C1 Bearer＋s1 输出无 Key＋t1 messages 仅用户输入）；N02 已有（debug 脱敏单测）；N03 已有（audit 报告无秘密值）；B01 已有＋补强（truncate_boundary 299/300/301 前缀精确）；B02 已有（中文计数＋空凭证路由拒绝）；B03 **新增通过（变体补齐）**（
 已有；secret_b03 补 CR/NUL：发送前拦截、零请求、不回显原值）；E01 已有（key_leak_scan 五场景不含 Key/指纹）；E02 **新增通过**（api 三位置＋真实 CLI 双流扫描＋**收尾轮横跨边界片段检测**，两处修复见缺陷表）；E03 部分覆盖（完整原串三位置＋横跨片段已测；转义/多字段嵌入**未展开**——文档本身不要求屏蔽任意短子串）；E04 已有＋E02 real CLI 补强（错误经真实输出扫描＋超限/头部拒绝复用）。

**8 TERM（10）**：N01 已有（normal_text_preserved）；N02 **新增通过**（charset 混合前后缀保留）；B01 已有（悬挂 ESC/纯转义）；B02 已有（OSC52/BEL/ST/标题）；B03 **新增通过**（charset nF 三字节；DCS/C1 单测已有）；B04 已有（未闭合 OSC/CSI 打断有界）；B05 部分覆盖（大输入为电池 D/F3 **一次性记录**）；E01 **新增通过**（term_e01 真实 CLI 零 ESC 字节、正常文字保留）；E02 已有（唯一出口 say；E01 实测回复路径＋s1 /models）；E03 已有（打断/截断单测＋id 前 8 截取）。

**用例状态合计（111）**：已有覆盖 **80**｜新增通过 **20**｜部分覆盖 **7**（AUTH-N02 人工、NET-B05 源码推理、NET-E03 人工、PATCH-B06 源码推理、PATCH-B08 人工、SECRET-E03 未展开、TERM-B05 人工）｜待决策 **3**（AUTH-B05、EXEC-P01、EXEC-P02）｜环境受限 **1**（NET-N01）。
**新增测试函数合计（24，与状态分开计）**：首轮 19＋收尾轮 5（auth_e01、net_b02、net_b06、secret_b03 补、secret_e02_straddle）。

## 六、命令与退出码

- 首轮：基线 `cargo check`（0）/全量（0，190/0/0）→ 终态（0，209/0/0）。
- 收尾轮：定向 `--test llm_flow secret_e02|auth_e01|net_b02|net_b06|secret_b03` 全过（0）；
  `cargo check` 0 警告（0）；`cargo test --workspace --no-fail-fast`（0，**214/0/0**：
  85 lib＋32 apply_patch＋9 e2e＋14 exec＋51 llm_flow＋9 prompt＋14 tool_loop）。
- 定向：`--test llm_flow secret_e02|net_b01|term_e01`、`--test e2e_acceptance auth_`、
  `--test exec_command exec_b04|b05|b07|p01|e03`、`--test apply_patch patch_b05`、
  `--lib turn::tests::loop_b05|display::tests|net_b03|auth_b05|trust_b0`——全部通过（0）。
- exec_b04 稳定性：单独连跑 10/10 通过。
