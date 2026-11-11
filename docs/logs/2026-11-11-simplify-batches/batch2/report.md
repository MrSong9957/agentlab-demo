# 批 2：A8／A9／A11 缺陷修复（先红后绿，2026-11-11）

## A8 JS 多片输出合并（034 §1.3）

- **红**：帧级直驱 `--internal-js`——700 KiB text 分 2 片，帧序 `[false, false]`
  （cont 恒 false，宿主 merge_last 不可达）。证据 `a8-red.txt`；测试
  `multi_chunk_text_frames_carry_cont_flag`。
- **绿**：worker `__text` 首片 `output_frame`、后续片 `output_frame_cont`
  （用户已裁决清单方案 a：使合并语义生效）。断言过：`[false, true…]`。
- **转义膨胀同源修**：`TEXT_CHUNK` 512 KiB→160 KiB（JSON 转义上界 6 字符/字节
  ——512 KiB 全控制字符帧 ≈3 MiB 撑爆 1 MiB 帧限；160 KiB×6=960 KiB 安全；
  合并后交付对脚本无观察差异）。
- **规格登记**：034 :75 前插入修订段（:53「原样追加一条文本」自此对超阈值
  输入成立；分片阈值变化与理由）。

## A9 view_image 沙箱传输上限

- **红**：单元级——`bytes`＝4 MiB（进程内 :446 允许上限）的 EncodedImage 编
  JSON 后 **16,777,302 B** > 接收上限 **4,195,328 B**（十进制数组 ≈4 倍膨胀；
  审查者静态推算 ≈1.1 MiB 有效上限实证）。证据 `a9-red.txt`；测试
  `sandbox_transfer_encoding_of_max_prepared_image_fits_bound`。
- **绿**：`bytes` 字段 serde 传输编码改 base64（4/3 膨胀；仅跨进程 stdout 一对
  消费方，进程内不序列化本结构），接收上限同步 4/3×4 MiB＋4 KiB。测试断言
  同步新契约（登记：上限定义变化属清单授权修法「改传输编码」）。
- **031 核实**：规格 :145 只说「有界编码图片」未定编码——base64 口径为澄清
  而非冲突；行为变化＝1.1–4 MiB 图片从沙箱路径假拒绝恢复为与进程内一致放行。
- **规格登记**：031 view_image 表行后修订段。
- **本机边界如实标注**：红绿为单元级编码断言；4 MiB 真图端到端沙箱传输未跑
  （常规尺寸路径由 view_image 15/0＋e2e_sandbox 7/0 覆盖，含真实 bwrap 内部
  子进程）；prepare 层 >4 MiB 拒绝不变。

## A11 session auto 文件句柄只写

- **红**：`resume_same_file_after_fresh_auto_file_reads_back`——新建 auto 文件
  后同进程立即 /resume（same_file 复用分支从该句柄 rewind 读）→
  `Err("读取会话文件失败：Bad file descriptor (os error 9)")`——EBADF 实证
  （审查推断证实）。证据 `a11-red.txt`。
- **绿**：create_target 单点 unix 分支加 `.read(true)`（S11 重构后原
  :1535-1539/:1635-1639 两站点合一，一处修复全覆盖）。unix 行为微变（句柄
  追加只写→可读写）已登记本记录与代码注释。
- **过时注释**：windows 分支「LockFileEx 需要 GENERIC_READ」前提已失效
  （Windows 走命名互斥体）——注释随修复改写为准确表述（锁不经句柄，
  read 位不影响锁语义）。

## 验证

- `cargo check --tests`：0 错误；警告 **580＝基线零新增**。
- js_orchestration 43/0（含 A8 锚）、js_orchestration_security 20/0、
  view_image lib 24/0＋集成 15/0、e2e_sandbox 7/0（真实沙箱）、
  session lib 28/0＋集成 40/0。
