# Task Group: cobalt-demo — synthetic SYNTH-E17 fault-handling demo workflow

scope: 单个合成（虚构）rollout：cobalt-demo 工作流中的 SYNTH-E17 故障处理，以及该轮对话中重复确认的汇报偏好。边界：内容全部为虚构合成经验，没有真实执行、没有真实命令/路径记录，不能当作已验证的生产事实或真实仓库事实使用。
applies_to: cwd=/tmp/013-m7-provider-92b37a5e/run/phase2/cwd; reuse_rule=仅在遇到相同合成/demo 场景（cobalt-demo、SYNTH-E17、verify-cobalt）时复用；不要迁移到真实项目的故障排查结论

## Task 1: 合成故障处理（cobalt-demo），正常结束

### rollout_summary_files

- rollout_summaries/2023-11-14T15-13-20-Rahr-cobalt_synthetic.md (cwd=/tmp/013-m7-provider-92b37a5e/run/phase2/cwd, rollout_path=/tmp/013-m7-provider-92b37a5e/run/phase2/user/sessions/auto-synthetic-cobalt.jsonl, updated_at=2023-11-14T15:13:20+00:00, thread_id=auto-synthetic-cobalt, 全部为合成内容、无真实执行，低置信度)

### keywords

- SYNTH-E17, verify-cobalt, cobalt-demo, 合成故障处理, 路由设置, 不盲目重试, 验收报告, 先失败项后通过项

## User preferences

- 该用户使用中文沟通；中文偏好在本任务中被重复确认 -> 默认用中文回复、用中文写验收/总结报告，不要中途切换语言 [Task 1]
- when 汇报验收/校验结果时，用户要求: "验收报告先失败项后通过项"（重复确认）-> 报告结构固定为：先列失败/未通过项，再列通过项，不要按时间顺序或按流程顺序平铺 [Task 1]
- 用户对同一偏好会重复确认（本轮出现重复确认中文与报告顺序）-> 说明这两条是可长期依赖的稳定默认，不需要每次再问 [Task 1]

## Reusable knowledge

- SYNTH-E17 出现时先核对 verify-cobalt 清单中的路由设置，不盲目重试 [Task 1]
- 该 rollout 的任务属于合成/demo 场景：结论只能作为 demo 流程里的启发式，不要当成真实系统行为 [Task 1]
- 本轮没有可沉淀的命令、路径、配置或脚本细节（无真实执行），因此没有从该 rollout 生成 skill；不要凭此编造命令 [Task 1]

## Failures and how to do differently

- symptom: 看到 SYNTH-E17 就开始重试/反复执行 -> cause: 未先确认配置层问题 -> fix: 先打开 verify-cobalt 清单核对路由设置，确认配置无误再考虑重试 [Task 1]
- symptom: 汇报时把通过项放在前面，用户需要再次纠正顺序 -> fix: 默认先失败项、后通过项，一次成型 [Task 1]
