# 032 变异反证记录（C18）

- 日期：2026-10-28；方法：临时注入环境变量门控的定向突变 → 定向用例红 → 还原 → 复绿；突变全部经 grep 确认移除（`grep -rn MUTATION src/app/src/` 零命中）。
- 命令：`env -u TCHAT_SANDBOX TCHAT_CM32_MUTATION<i>=1 cargo test --locked --test context_management <定向用例>`。

| # | 突变（注入点） | 被攻击行为 | 用例红证据 | 还原后 |
|---|---|---|---|---|
| 1 | `active_view` 忽略检查点回退全历史 | 活动视图选择（归档前缀不重发） | `m4_switch_happy…` panics「归档前缀不得重发」 | 28/28 绿 |
| 2 | `commit_context_switch` 忽略宿主回执失败仍推入检查点 | 提交回执（scheduled≠applied；失败候选不复活） | `m4_commit_failure…` panics「未应用通知」（实际收到"已提交"通知） | 28/28 绿 |
| 3 | new_context 排定即 `requests=0` | 请求计数（切换不重置 8 次额度） | `m4_eighth…` panics「不发第 9 次请求 left:9 right:8」 | 28/28 绿 |
| 4 | `rollout_input` 版本门拒绝 V3 | 记忆版本门（下游读取器同步支持 V3） | `m3_rollout…` panics「V3 可读: "MUTATION-4"」 | 28/28 绿 |

备注：突变 3 首版置于批末提交处未生效（quota_hit 已提前快照）——移至排定点后按预期触发；这本身验证了「额度判定先于提交快照」的实现顺序。
