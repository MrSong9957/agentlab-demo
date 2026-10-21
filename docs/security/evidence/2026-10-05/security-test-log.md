# 安全攻防测试日志（2026-10-05）

## 关键设计依据

- 被测对象无 Web 服务端，攻击面 = 出站 HTTP（chat/list_models/sync_public）+ 本地配置/凭证层 + 终端输出。用户 8 类清单按"等价攻击面"映射（见报告第 4 节），无法映射的注明不适用。
- 攻击脚本 `attack.py`：原始套接字 mock（逐字节记录请求）+ 真实二进制管道驱动；独立临时配置目录 + 伪造密钥，符合 AGENTS.md 第 3 节隔离要求。

## 测试要点

- 高危通道：配置投毒→Key 外带（F1）、`file:` 凭证引用→任意 JSON 字段外带（F2）、ANSI 转义注入三通道（F3：回复/错误/目录元数据）、公共目录明文 http 投毒（F4）。
- 防护断言：远程 http 拒绝、保留字段拦截、能力未知参数拒绝、CRLF 头注入拦截、分页 100 页上限、缺失密钥不发包、全局输出无密钥明文。

## 失败原因与修复（harness 缺陷，非被测程序问题）

1. **首轮 C0/C8d 假象**：mock 在"头部+正文同段到达"时未消费剩余缓冲即等待正文 → 30s 超时断连，客户端报 `Unexpected EOF`。修复：`body, buf = buf[:clen], buf[clen:]` 后再续收。修复后 C0 请求正常到达且结构完好、C8d 实测 `max_tokens:-5` 出站。
2. **C4 误判**：原以为 300 字符截断覆盖 `error.message`；实测 `api.rs:46` 截断仅作用于"非 JSON 响应体"，`error.message` 分支全量回显（320 个 X 全部出现）→ 结论从"回显"升级为"无上限全量回显"。
3. **C12 误判**：serde_json 把 ESC 存为 `\u001b` 文本，用原始字节匹配落盘文件导致误判"未投毒"；改为两种形态都匹配后确认投毒落盘 + `/models` 二次回显。
4. **C5 判定收紧**：CRLF 被 URL 解析器剥离（`after=abcX-Injected:%201`），未构成 HTTP 头注入，从 VULN 降级 INFO；`&` 参数注入维持确认。

## 验证结果

- 34 用例全部落盘 `results.json`；全局泄露断言通过（伪造密钥/受害者字段/真实 .env 密钥均未出现在任何输出）。
- C6 上限实测：恰好 100 次 `/models` 请求后中止；C7 内存线性放大：11MB→峰值 29MB、30MB→68MB。
- 未修改产品代码；`cargo build --locked` 通过。报告见 `docs/security/reports/2026-10-05-terminal-chat-api-security.md`。

## 修复阶段记录（同日）

- F5：`list_models` 改用 `Url::query_pairs_mut`；红测试证明 `&` 拆参，修复后 `a&b#c模式` 单参数无损。
- F6：`LimitedReader` 按实际字节限 10MiB（含错误路径与公共目录）；恰达上限边界有单元测试。
- F3：新增 `display.rs`（CSI/OSC/DCS/SOS/PM/APC/两字符转义 + C0/C1/DEL 全剥离）；`error.message` 统一 300 字符；main.rs 全部不可信输出经 `say()`。注意 OSC 载荷整体剥离属正确行为（曾误判为 bug）。
- F2：`file:` 凭证限定 `user_dir()/credentials`（canonicalize 真实路径包含校验）；测试凭证迁移进专用目录，`unique_cred_file` 加 TCHAT_USER_DIR 防呆（曾误写 ~/.config/terminal-chat，已清理）。
- F7：`ensure_private_dir`/`write_private_file`/`audit_file_permissions`；启动审计收紧真实 .env 与 config/*.json 为 0600（实测生效）；上级目录可写只告警；`write_atomic` 临时文件 0600。
- F1：授权记录（origin+凭证指纹）存用户私有目录 0600；非交互未授权拒绝；`redirects(0)` + 3xx 拒绝；chat/refresh 全部带凭证请求过门。
- F4：公共目录 URL 校验（HTTPS 必须，回环 http 仅 `TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK=1` 测试开关）+ 目录来源授权（cred_fp="catalog"）+ id/名称入库校验。模型 id 规则：拒绝控制字符/空白/超长，允许非 ASCII 可打印（与 F5 中文游标兼容）。
- 回归：cargo check 无警告；cargo test 89 通过；pytest 2 通过；attack.py v2 37 条全过（防护/正常/信息，0 漏洞）；全局泄露断言通过。
- v2 脚本调试要点：C13 需在真实工作区 cwd 运行（输入仅 exit，无网络）；PTY 会话中 token 匹配必须增量扫描（旧 token 会提前命中）；C2 合法引用的授权指纹须绑定文件凭证实际值。
- 修复记录报告：docs/security/reports/2026-10-05-terminal-chat-api-security-fix.md

## 定向收尾记录（同日，第三轮）

1. **数量与证据对齐**：最终 `results.json` 38 条 = 原 34 条（逐 ID 核对齐全：C5 六项、C7 两项、C9 八项）+ N1～N4。37→38 原因：修复报告初稿写于补回 `C9-https 任意主机（无白名单）` 之前。修复报告附录 A 已逐 ID 列出，脚本核对与 `results.json` 的 ID+判定完全一致；v1 修复前证据归档 `v1-prefix-evidence/`（44 项），`results-prefix-fix.json` 为修复前结果。
2. **F4 测试例外收紧**：收尾审查确认正式二进制此前仍可经 `TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK=1` 放行本机回环 http——不满足"仅用于测试"。已从产品代码**完全移除**该开关；改为 `sync_public`/`refresh` 的 `url_override` 参数测试注入（`#[doc(hidden)]`，正式二进制固定传 None）。验证：集成测试 `production_catalog_path_rejects_plaintext_http_even_with_stale_switch`（旧开关存在也拒绝、明文目录 0 访问、旧目录保留）；单元测试确认设置旧开关变量不改变判定；E2E C12 两场景（带/不带开关）均拒绝且 0 访问。本机模拟测试能力保留（集成测试注入）。
3. **F7 收口**：`apply_mode` 设置后**重新读取元数据确认**，未生效报"权限设置未生效：期望…实际…（挂载环境可能不支持）"；`write_private_file` 对已存在文件显式收紧（OpenOptions mode 仅创建时生效的缺口已补）；Windows 的"继承 ACL 仅当前用户可访问"说法撤回——运行时输出未验证告警，交叉编译检查因环境无法下载 Windows std 未执行（未验证）；`/workspace`、`/tmp` 实测仍 world-writable，审计每次告警，风险保留未消除。
4. 回归：cargo check 无警告；cargo test 90 通过（41+49，新增正式路径拒绝测试）；pytest 2 通过；attack.py 38 条全过；泄露断言通过。最终 `results.json` 为权威证据。

## 定向核对记录（同日，第四轮：C8b/C8d 归类与 /tmp 权限）

1. **C8b/C8c/C8d/C10 核对——发现 harness 预置缺陷（非产品问题）**：最终一轮中 C8 三个子用例复用 8a 的授权目录而 mock 端口不同、C10 用空授权目录，F1 授权门先行拦截 → C8b"无覆盖"未经真实请求、C8c/C10 的 0 请求原因混淆、C8d 的 PASS 为假阳性（max_tokens=None）。修复 attack.py 预置（各自预授权/隔离密钥检查），断言只增不减；C8b 经 `supported_params=["Model"]` 注入实现线级验证（请求体 `{"Model":"attacker-model",…,"model":"deepseek-flash"}`）；C8d 真实直发 `-5/42`，判定 PASS→WARN；C8a/C8c/C10 增加拒绝原因断言。定向补跑合并写回 results.json（仍 38 条）。原报告记载与 v1 证据一致，无需更正；修复报告附录 A 的 C8d 判定已更正并新增附录 B 更正记录。
2. **/tmp 权限判断——审计告警在该场景保守过严，记录更正**：`/tmp`=1777（sticky），他人不能替换属主目录，程序临时目录 0700 + 产品敏感文件 0600 → 实际无暴露；`/workspace`=777 无 sticky → config/.env 可被替换（完整性风险真实，泄露因 0600 消除）。修复报告剩余限制已按场景区分；审计考虑 sticky 位属产品改进项，本轮未做。
3. 本轮仅改 attack.py 与记录文档；产品代码、Rust/pytest 断言、目录权限均未动，未重跑全套（仅定向补跑 C8/C10 并合并）。
