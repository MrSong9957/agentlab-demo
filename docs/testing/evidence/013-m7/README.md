# M7 供应商补验核心证据

M7 代码接线及约定补验证据已交付。本目录保留便于直接审阅的原始捕获；完整过程材料、历史源码和原始 manifest 位于 Git 快照 `9e32a63`。所有迁移文件保持原字节，位置及 SHA-256 见[逐文件留存清单](../curation-manifest.json)，取回方法见[统一索引](../../../records/evidence-index.md#m7-originals)。

| 位置 | 内容 |
|---|---|
| [final/summary.json](final/summary.json) | 最终三批各自结果、实际请求及原始 usage 汇总 |
| [final/control/execution-summary.json](final/control/execution-summary.json) | 一次执行的串行计数、退出、67,487 ms 整体计时及封批结果 |
| `final/batches/` | 三个独立根的身份、封闭账本、全部8次真实请求／响应、工具事件、产物、SQL及合成会话 |
| [final/audit/runtime-results.json](final/audit/runtime-results.json) | 非空字段、三工具7次成功、SQL／Git基线及注入不入史核验 |
| [authorization-package.json](authorization-package.json) | 当时授权范围、1／8／1与累计10限制、已审源码指纹 |
| `historical/` | 0次失败、1次合法空、三批未执行的原始判定与各自封闭台账 |
| `validation/` | 最终离线检查、变异证明及关键失败／修复依据 |

最终三批实际发送1／6／1，全部HTTP 200并分别达到目标；phase2余2次封存。usage为97,660输入＋6,120输出＝103,780 tokens，费用未知；内部重试0、后台额外触发0。phase1为`SucceededWithOutput`、恰一行非空输出；phase2三工具成功、两产物及`v1`、SQL和既定Git基线合同通过；注入正常回复含COBALT-17、自动记忆不入史。

捕获JSON中的“待审阅”、授权包中的“待授权”、旧临时路径及相对路径均是执行时原文，不表示当前尚未交付或允许续跑。解析其中路径时应使用留存清单的`original`→`destination`映射；仅在快照中的文件按原路径取回。没有修改任何历史账本、usage、原始指纹或原始附件中的空白。

三份合成夹具独立；同链端到端、system/developer语义等价、其他供应商／模型及Windows未据此验证。M8/M9未实施，V1未完成。实际测试源码及历史命令由原始快照保存，本目录不提供新的发送入口。
