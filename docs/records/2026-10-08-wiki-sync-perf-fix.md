# wiki_sync MCP 30s 超时修复（2026-10-08，diagnosing-bugs 流程）

症状：ZCode MCP `wiki_sync` 客户端 30s 超时；CLI 增量同步实测 123.8s（CPU 仅 ~10s）。
修复：增量同步 123.8s → **2.2s**（约 56×），MCP 协议层验证不再超时。
代码：`/vault/mcp/wiki-query/src/sync.rs`（+33/-12，**未提交**，待用户 commit）。

## 诊断链（回路 + 微基准分阶段排除）

1. 回路红灯：`/vault` 增量同步计时 > 30s 判红 → 123.8s，且 **0 文件变更**（时间不在写入路径）。
2. 微基准三分解：目录遍历 0.3s、逐文件 stat 1s、**SQLite 查询 122s**——瓶颈锁定查询本身。
3. 反直觉对照：库拷到本地快盘同样 123s、换 DELETE 日志同样 120s、同参数重复查询仅 1ms → 排除文件系统/WAL，指向算法。
4. 定性证据：`EXPLAIN QUERY PLAN` 显示 `SELECT count(*) FROM documents_fts WHERE path=?` 是 **SCAN documents_fts VIRTUAL TABLE**（FTS5 对 path 无二级索引，每次全表扫描）；分项计时 documents 查询 0.8ms/条 vs fts count **218ms/条**；503 文件 × 218ms ≈ 110s，与 123.8s 吻合。

根因：`needs_update()` 逐文件做 FTS 缺行检查，O(N²)。

## 修复

sync 开始时一次 `SELECT path FROM documents_fts` 装载 HashSet（单次全扫），`needs_update` 改 O(1) 成员检查。语义完全保留：仍能发现 FTS 缺行触发自愈（失败时空集 → 全部判需更新，与原 count 失败语义同向保守）。

## 验证

- wiki-query 全套测试通过（20+40+7+12+12+8+40+4+2+6）；其中 FTS 自愈守护 `fts_gap_from_interrupted_write_self_heals` 修复版通过。
- `sync_boundary_test` 与原版代码基线对照**零新增失败**（migration_dup 在 Linux 上环境性失败：上游按 Windows 反斜杠语义设计；其后 4 个失败是其引发的 cwd 级联——首个失败测试跳过 teardown，进程 cwd 留在已删除临时目录）。
- 回路两次 GREEN（2.2s/2.3s）；MCP 协议层 `wiki_sync` 新实例正常返回完整报告；删库全量重建后搜索正常。

## 边界与遗留

- 全量重建（503 文件全写＋2212 对查重）仍 ~2 分钟：每文件事务 fsync＋查重 MATCH 主导，属首次建库场景，量级与修复前相同，不在本次范围。
- ZCode 当前连接的 MCP 服务仍持旧二进制 inode（替换时 Text file busy，已 rm+cp 放新文件）——重连/重启会话后生效。
- 容器适配两处：e2e 套件需 `mcp/wiki-query/target/debug/wiki-query.exe`（上游写死路径，已放 debug 产物，gitignored）；/vault 的 git 操作受 root 目录属主限制，文件级覆盖可用（src 已 chown dev）。

建议 commit message 附根因："fix(sync): needs_update 的 FTS 缺行检查逐文件全表扫描 O(N²)（503 文件 124s），改为预装载 path 集合 O(1) 成员检查，增量同步 124s→2.2s"。

## 收尾验收（2026-10-08）

- code-review 双轴（子代理并行）：无硬违规、无规格缺失；三项保持语义（自愈/保守失败方向/mtime 不漂移）逐点核验通过。范围内 finding 已修：装载失败补警告日志（失败半径从单文件扩大为整轮，须可观察）。判断级 smell（加载形状重复、scan_directory 8 参数）记录跳过：既有模式，抽取会扩散改动超出最小修复。
- 真实 ZCode 连接验收：杀旧进程后 ZCode 自动重启（/proc/exe 确认新二进制）；无变化同步完整报告返回（此前同调用 30s 超时）；临时文档 updated=1 且唯一令牌检索命中 1 条；再次同步 updated=0 幂等；清理 pruned=1 后令牌检索为空。五个 MCP 工具全部真实连接验证通过。
- 终态测试：全量套件 151 通过；唯一失败 sync_boundary_test（migration_dup 平台语义 + cwd 级联），单跑与整组均与原版基线一致，无新增失败。CLI 增量终版计时 2.08s。
- 提交：`998f4b8` @ 分支 `fix/wiki-sync-fts-fullscan`（仅 sync.rs，+37/-12，未 push）。工作区剩余：19 个 0 行差异幻影修改（既有跨平台状态）＋未跟踪 wiki/1.md（用户上次合并后的残留），均不属于本次修复。
- 结论限定：日常增量同步超时已修复；全量重建（503 文件全写）仍 ~2 分钟、可能超 30s，当前用 CLI 跑。

