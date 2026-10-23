# 计划：查看图片工具 view_image 最小接入

> 状态：**已实施（2026-10-23）**，全量验证通过（3229 通过 / 0 失败；基线 3175）——实施证据、TDD 红绿记录与变异证明见[实施记录](../records/2026-10-23-view-image-impl.md)。落点差异：§6 所列 `tool_loop`/`session_cli` 未单独扩展，取消/合并轮/恢复等行为统一纳入新增 `tests/view_image.rs` 与 `prompt_assembly`/`session`/`e2e_acceptance`（覆盖等价）。
> 日期：2026-10-23。
> 依据：[工具能力对比](../records/2026-10-21-codex-tools-comparison.md)中的 `view_image` 缺口；源码与本地约束见[调研记录](../records/2026-10-23-view-image-plan-research.md)。
> Codex 基线：`d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`，沿用对比文档的固定提交。

## 1. 目标与范围

补齐一条完整链路：**模型调用 `view_image({"path":"截图路径"})` → 宿主读取并处理图片 → 下一次模型请求携带真实图片 → 模型继续原任务 → 保存、恢复后仍可使用当时的图片内容**。

只增加 `view_image` 一个工具。复用 Codex 的参数 Schema、图片识别与缩放函数、data URL 编码和图片历史卡片；只适配本项目的 Chat Completions、工具循环、模型目录和 V1 会话格式。不整套引入 Codex core、执行服务器或附件存储服务。

首版只处理磁盘上已有的图片。截图拍摄、浏览器操作、`-i` 参数、剪贴板、拖入、`@` 选图、图像生成、远程 URL、OCR 专用工具和终端内图片预览均不在本次范围。默认采用上游 `high` 准备策略；不开放 `original` 和多环境选择。

成功标准是模型请求中确实存在可解码的图片内容。路径文本、base64 字符串说明或界面上的“已查看”提示均不能单独作为交付证据。

## 2. Codex 复用清单

下表均固定在上述提交。实施时记录函数来源、许可证和逐项适配；优先恢复仓库已保留的代码。

| 复用部分 | Codex 源码 | 最小适配 |
|---|---|---|
| 工具名称、描述与路径 Schema | [view_image_spec.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/view_image_spec.rs) | 取三个选项均为 `false` 的基础参数面，改为本项目的 Chat Completions function 包装；必填项只有 `path` |
| 路径解析、普通文件检查、错误分类与图片结果语义 | [view_image.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/view_image.rs) | 用本地文件读取、取消标志及结果记录替代上游 environment、sandbox context、TurnItem 和 ToolOutput 框架；图片载荷与可见摘要分开 |
| 图片解码、缩放、格式编码与 data URL | [utils/image/src/lib.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/utils/image/src/lib.rs)、[image_preparation.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/image_preparation.rs) | 函数级移植 `EncodedImage` 所需数据、未缓存处理路径、`HIGH_DETAIL` 尺寸计算、编码及元数据处理、`data_url_from_bytes`；省去缓存、附件上传、data URL 输入解析及其他 detail 模式 |
| 模型图片输入能力 | [openai_models.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/protocol/src/openai_models.rs) | 复用 `input_modalities` 及 `text/image` 取值；本项目能力未知时关闭图片入口，区别于上游缺字段默认允许 text＋image |
| 查看图片历史卡片 | [history_cell/patches.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/history_cell/patches.rs) | 优先复用本地已保留的 `ViewImageHistoryCell`，恢复被裁掉的构造入口；补活动 ID 和本地路径适配，不另写一套卡片 |
| 行为测试 | [core/tests/suite/view_image.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/tests/suite/view_image.rs)、[image_tests.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/utils/image/src/image_tests.rs) | 移植图片内容、尺寸、无效文件、缺失路径及文本模型用例，外部协议替换成本地固定 HTTP 响应；不用上游网络和远程环境测试装置 |

图像处理放在拟新增的 `src/app/src/view_image.rs` 内，不为单一调用新建通用多模态框架或缓存 crate。直接使用工作区已锁定的 `image 0.25.10`、`base64 0.22.1`；编解码特性沿用 TUI 的 JPEG、PNG、GIF、WebP。必要时只在 app 中补直接依赖并同步 lockfile，不升级工作区依赖。

## 3. 工具契约与模型能力

### 3.1 输入与文件读取

```json
{"path":"artifacts/screenshot.png"}
```

- `path` 为必填字符串。沿用上游 `strict:false`、`additionalProperties:false` 的声明和 Serde 参数解析；不把 Schema 当作宿主类型校验。裁掉的可选功能不进入首版接口。
- 相对路径以当前会话工作目录解析；绝对路径按原义使用。保持空格和中文路径，不作 shell 展开，不启动外部查看器或命令。
- 先确认打开的对象是普通文件，再有界读取；Linux 打开路径时避免 FIFO 阻塞，并对已打开句柄复核文件类型。目录、设备、FIFO、缺失和不可读路径返回明确错误。符号链接按宿主既有读权限解析到普通文件，不新增“只许工作区内”的权限规则。
- 根据实际字节识别格式并完成解码，不能只看扩展名或 MIME。非图片内容不得以 data URL 回传。SVG、PDF 不属于首版解码范围。
- 保留上游 PNG/JPEG/WebP 的可用源字节与必要元数据处理；GIF 按上游解码后转换为静态 PNG。缩放使用上游 `HIGH_DETAIL`：最大边 2048、32 像素 patch、最多 2500 patches，保持比例、不放大；不自行发明压缩算法。
- 在读取前、读取后和结果提交前检查取消。图像解码使用有界同步处理，不增加后台任务；解码途中取消在下一检查点生效，不宣称能瞬间中断解码器。

建议作为首版固定资源边界：输入文件最多 **20 MiB**；解码前限制宽、高各不超过 **16384**，并设置解码器 **64 MiB 分配预算**；准备后的图片字节最多 **4 MiB**，超过即返回工具错误，不循环降低质量。这些是本项目的资源限制，不是上游或提供方的 API 限额，也不代表进程总内存上限。

### 3.2 模型能力

- `catalog::ModelInfo` 增加可选 `input_modalities`，用户可在现有模型配置对象内明确填写 `{"input_modalities":["text","image"]}`。只复用 `text/image` 子集，不带入音频功能和完整上游模型目录。
- 缺字段表示未知，首版不声明、不执行 `view_image`；只含 `text` 同样关闭。此默认值是适应多提供方项目的主动差异，不能沿用 Codex 的默认可看图假设。
- 字段合并沿用现有逐字段覆盖；`/refresh` 不覆盖手动确认值，不根据模型名称猜测能力，也不新增探测请求。
- 能力在每个用户轮固定快照，同时用于工具声明与执行分派。无宿主问答、同步问答、异步问答三种用户轮按各自原工具面加一项；内部记忆合并轮继续只开放原三项 `mem_*` 工具。
- API 发送前再次检查图片消息与所选模型能力。恢复图片历史后切到文本或能力未知的模型，返回明确的本地错误，提示改用支持图片的模型或新建会话；不静默丢图，也不发送不支持的请求。
- 旧模型配置保持可读；新增字段实际写入配置后，旧版本的 `deny_unknown_fields` 解析器会拒绝该配置，回退旧程序前需移除该字段。实施时补配置兼容性用例和使用说明。

### 3.3 结果与循环

成功结果分成两部分：

1. **文本摘要**：沿用 `ToolResult.content: String`，返回含 `ok/error`、已解析路径、源尺寸与发送尺寸的有界 JSON；失败为 `ok=false` 和具体原因。
2. **图片载荷**：同一 `ToolResult` 增加可选 `image` 字段，结构保留上游 `image_url`、`detail` 的含义，首版 `detail` 固定为 `high`。只有成功加载时存在，data URL 只放此字段。

`ToolEnd.result` 继续只传文本摘要，不复制图片载荷到通知、活动行、详情或日志；`ToolDisplay` 仍只用于展示，不承载发给模型的图片。成功仅表示图片已装入本地历史；后续请求超限或失败时应单独报告，不能据此声称模型已经收到或理解图片。

保持调用 ID、工具名原样、串行执行、每轮 8 次模型请求和每批 32 条调用上限。取消后未开始的调用、参数错误和读取失败都补齐配对结果，且不附图片。第 8 批仍按既有规则执行并保存，不发第 9 次请求；下一用户轮可使用已保存的图片。

## 4. Chat Completions 的必要适配

Codex 此提交把图片放在 `function_call_output.output` 的 `input_image` 内容项内；上游测试还明确要求不另插图片消息。本项目不能照搬其 wire format。[OpenAI Chat Completions 接口](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)的 tool 消息只支持文本，user 内容部件才支持 `image_url`。

因此只在请求投影层作如下适配；这是本地设计，不声称与上游角色语义等价：

```text
assistant：tool_calls = [view_image A, exec_command B]
tool：A 的文本摘要，tool_call_id 原样
tool：B 的结果，tool_call_id 原样
user：宿主标注的 A 图片附件说明 + image_url(data URL, detail=high)
```

- 先输出当前批全部配对 tool 结果，再按原调用顺序输出成功图片的附件消息，之后才接原有后续消息。不能在 A 与 B 的 tool 结果之间插入 user 消息。
- 附件说明标注来源是 `view_image` 工具结果、关联调用 ID、路径及实际尺寸；图片及图中文字是待分析材料，不构成新的用户指令。不改 system/developer 权限层级。
- 附件消息由历史投影生成，不额外写成 `HistoryItem::Text`，避免恢复时重复、破坏工具配对或伪装一次用户提交。一次投影中每个成功结果只附一次；后续请求依照完整历史自然再次携带。
- `project_history` 需要从逐条一对一映射改成按批输出；只为图片分支缓冲附件。没有 `image` 的历史必须保持现有请求形状、内容和顺序。
- 保持 `api::MAX_REQUEST_BODY_BYTES = 10 MiB`，仍对最终 JSON（含 model、tools、历史、base64 与 request_params）统一计量，超限零发送。多张图片累计可能使后续请求超限，首版明确报错，不自动裁历史或提高上限。
- 沿用现有提供方请求和错误通道；服务端拒绝图片时如实报错，不自动退回纯文本或重试。若错误响应回显 data URL，先遮蔽图片载荷再截断输出，保留现有凭证脱敏和重定向限制。

## 5. 会话、记忆与界面

### 5.1 保存与恢复

- `ToolResult.image` 使用默认缺省、为空不序列化的可选字段，保留 V1 meta 与 `content/display` 原语义。纯文本会话序列化不增加字段。
- 保存的是**准备后、实际用于模型请求的图片**，内联于原结果行，不仅保存路径；不增加附件目录、数据库、路径重读或文件同步。原截图随后被覆盖或删除，恢复请求仍用当时字节。
- 保留 007 的追加、锁、原子快照、64 MiB 行上限和配对修复。图片数据、文本摘要及调用 ID 在同一结果行保存，避免崩溃时产生独立的孤儿附件。
- 恢复时只接受有界且有效的本地图片 data URL；远程 URL、非法 base64、错误 MIME、错误 detail 或超出图片限额的字段按 007 坏行规则处理，并补未知结果。不为校验加载原路径或访问网络。
- 新版本可读旧会话。旧版本会忽略新增 `image` 字段并在重存时丢失图片；该降级不可宣称无损兼容，须写入交付说明并验证。新版本读取此类只有成功摘要、无载荷的记录时明确显示“历史图片内容不可用”，不当成已附图。

### 5.2 记忆提炼

`memory::rollout_input` 目前与聊天共用 `project_history`。图片接入后必须先剥离 `image` 字段，再走记忆投影，保留工具文本摘要；不能让 base64 进入提炼文本、记忆 Markdown 或摘要注入，也不新增图片 OCR／视觉提炼轮。

保持现有单条 1 MiB、总量 8 MiB 的提炼读取预算。大于单条上限的内联图片记录仍按现有规则跳过，连同该行摘要一起不参与提炼；这是内联存储方案的已知取舍，不为本功能扩大限额或另建摘要副本。

### 5.3 用户看到的结果

- TUI 使用已有 `ViewImageHistoryCell`：摘要显示文件名，详情保留路径；需要时附文本尺寸说明。只恢复构造入口及必要活动 ID，沿用当前窄屏、详情与取消操作。
- 普通 CLI 保留活动行，并输出已加载路径、实际尺寸；失败和取消显示原因，不输出 data URL。
- 实时终态和 `/resume` 共用同一成功／失败转换。失败、未知结果、缺失图片内容不能生成正常“已查看”卡片；恢复只显示，不重新执行工具。
- 路径和错误在终端显示前复用 `sanitize_for_terminal`，只过滤显示副本；持久化参数、关联 ID 与模型实际图片数据不变。

## 6. 修改落点

| 文件或模块 | 计划改动 |
|---|---|
| `src/app/src/view_image.rs`、`lib.rs` | 函数级复用上游处理逻辑；参数、文件读取、限额、图片准备与有界摘要 |
| `src/app/Cargo.toml`、`Cargo.lock` | 补已有锁版本的 image/base64 直接依赖，不新增图像框架 |
| `src/app/src/catalog.rs` | 模态字段、逐字段覆盖与能力判断；必要配置文档同步 |
| `src/app/src/prompt.rs` | 用户工具条件注册、可选图片历史字段、按完整工具批投影图片附件 |
| `src/app/src/turn.rs` | 传入每轮模型能力；最小扩展工具执行返回值以携带可选图片；取消、入史和分派隔离 |
| `src/app/src/api.rs` | 图片能力发送前检查、错误回显载荷遮蔽；复用既有最终请求大小检查 |
| `src/app/src/session.rs` | 现有结果记录读写中保留新增字段；加载校验复用坏行及配对修复 |
| `src/app/src/memory/rollout_input.rs` | 记忆投影前清除图片载荷，保持既有预算 |
| `src/app/src/main.rs`、`tui_host.rs` | 当前模型能力传递、行式摘要、实时和恢复的图片结果转换 |
| `src/tui/src/chat_driver.rs`、`history_cell/patches.rs` 及相关测试 | 新增最小宿主图片项，恢复已有卡片构造入口与上游窄屏／路径测试 |
| 新增 `src/app/tests/view_image.rs`，扩展 `prompt_assembly`、`tool_loop`、`session`、`session_cli`、记忆与 `e2e_acceptance` 测试及相关快照 | 将表中行为纳入现有回归入口，优先移植上游测试，不另建测试运行框架 |
| 005／006／007／008／012 规格及配置说明 | 只更新本功能涉及的工具面、消息投影、图片卡片与兼容性约定；不清理无关内容 |

## 7. 实施顺序与验收

以下均为后续执行步骤，本轮不执行。

1. **确认基线并写失败用例 → 验证测试能够检出缺口。** 先记录相关原有测试状态，再按 TDD 补模型能力、无效输入、图片处理及多工具批次投影用例；保留首次失败证据。
2. **接通最小图片链路 → 验证真实请求载荷。** 复用处理函数、加入能力开关、执行分派和可选载荷，用固定本地 HTTP 服务验证下一请求的图片字节、MIME、尺寸及结果顺序，完成所有计划单元／集成测试。
3. **接通保存、记忆过滤与显示 → 验证可恢复且无重复。** 验证源文件删除后的自动恢复、手动快照、缺图降级、记忆输入无载荷、CLI 输出与真实 TUI 屏幕；相关新增测试继续先失败后通过。
4. **完成规定门禁 → 验证回归与最小修改。** 按 `comprehensive-testing → Security Testing → code-review → 全量验证和修复` 执行；任何阶段失败用 `diagnosing-bugs` 定位，复验受影响项，最终执行全量命令。将实际结果写入同一份实施证据，不用本计划中的待验收项代替通过证据。

| 验收面 | 必须观察到的行为 |
|---|---|
| 工具与能力 | image 已确认的各用户轮只加一项工具；text／未知及 MemoryMerge 在声明和执行两层关闭；`/refresh` 保留手动值；伪造调用不读取文件 |
| 参数与路径 | 非法 JSON、缺字段、错误类型、目录／设备／FIFO、缺失／不可读文件明确失败；空格、中文、相对路径、绝对路径和普通文件符号链接按约定工作 |
| 真实图片 | 固定 PNG/JPEG/WebP/GIF 经真实处理代码；请求图片可 base64 解码并由 image 重新打开，格式、尺寸及代表性像素与固定期望一致；小图字节保留与 GIF 转 PNG 的行为符合上游 |
| 缩放与边界 | 覆盖宽图、长图和 patch 限额；2304×864 的上游样例为 2048×768；大尺寸、输入／解码／输出限额前后边界能失败且不上传非图片字节；保留相关色彩／EXIF 元数据语义 |
| 请求批次 | 同批看图＋其他工具、同批两张图、先失败后成功、连续多批；tool 结果全部先配对，附件与 ID 及顺序准确，无重复 user 历史；纯文本请求黄金对比不变 |
| 生命周期 | 取消前、中、后的检查点及第 8 批保存；跳过的调用无图片、无额外读取；仍只允许 8 次模型请求 |
| 大小与失败 | 累计图片及附加参数计入原 10 MiB 请求限制，超限网络请求计数为零；提供方拒绝不重试、不降为纯文本；错误和普通日志无图片载荷 |
| 保存与恢复 | auto 追加、`/save`、`/resume`、崩溃残批、损坏图片字段；删除／覆盖源图后恢复请求字节仍等于先前图片；恢复不读源图、不重新执行工具；旧会话不变、旧程序重存丢图的降级有固定夹具证明 |
| 记忆隔离 | 正常尺寸图片结果保留文本摘要但无 data URL；超 1 MiB 记录仍按既有规则跳过；提炼与合并不获得新工具权限 |
| 用户最终结果 | 真实 CLI 进程完成看图后继续输出回答；PTY＋屏幕解析验证 TUI 成功／失败／取消、窄屏、详情路径、保存恢复一致；路径控制字符不能产生终端副作用 |

测试只在独立临时目录中使用固定图片与本地 mock HTTP，覆盖用户目录和相关环境变量；不访问真实配置、不使用真实密钥或在线视觉服务。稳定 JSON 和屏幕优先沿用现有快照；像素、尺寸与字节检查同时防止测试只匹配提示文案。离线验收证明图片到达模型协议边界，不能冒称真实模型的视觉理解质量已验证。

关键用例须保持断言不变，证明至少能发现：工具漏注册、输出只有路径而无图片、附件插入批次中间、缩放算法失效、恢复改为重读原文件、记忆过滤漏清载荷。使用修复前后对照或隔离副本中的可逆变异，不能通过删除测试、放宽断言或直接接受新快照过关。

在 `/workspace` 最终执行项目规定的 `cargo check && cargo test --workspace --no-fail-fast`；关键 CLI／PTY 用例纳入现有 `e2e_acceptance`，由全量命令自动执行。开发排查可用 `cargo test --test e2e_acceptance`，定向结果不能代替全量验证，全量通过后无新问题不重复跑同项检查。按实际工具链运行适用 lint，区分原有失败；报告通过、失败、忽略／跳过、未运行和过滤数量。若使用隔离副本，显式指定其专用 `--target-dir`。本项目没有已采用的公开 crate API 兼容检查工具，配置与 V1 兼容性以本表的历史夹具和读写用例验证，不强加新工具。

## 8. 本轮计划验证

本轮仅核对指定文档、本地实现、既有测试及固定提交源码，编写方案并检查文档命名、内容和链接。未修改程序、测试、依赖或锁文件，未注册 `view_image`，未运行 Cargo 检查或功能测试。文档检查结果统一记录于[调研记录](../records/2026-10-23-view-image-plan-research.md)。

## 9. CLI 图片输入补充讨论（2026-10-23）

本节原用户附图讨论已提炼为独立的 [020 CLI 用户附图规格](020-cli-image-input.md)。后续实现、范围与验收以 020 为准；本文继续约束已实施的 view_image 工具。019 §1–§8、[原实施证据](../records/2026-10-23-view-image-impl.md)与[调研 §5–§7](../records/2026-10-23-view-image-plan-research.md#5-cli-图片输入讨论落盘2026-10-23)保持原意，不用后续附图规格重写历史通过结论。

为保留已有记录中的链接，原 9.1–9.6 标题与锚点继续作为关联索引；不在两份规格维护重复契约。020 本轮仅作规格准备，功能尚未实施，全部功能验收待验证。

### 9.1 共六个使用入口，Codex 均已有实现

见 [020 §1.1：六入口现状与范围](020-cli-image-input.md#11-六种入口的现状与执行范围)。

### 9.2 第 2、6 项：共用看图能力，触发链路不同

见 [020 §2／§3：用户消息与工具结果的不同归属](020-cli-image-input.md#3-关键判断为什么)。

### 9.3 本项目还需接通的部分与建议顺序

见 [020 §1／§5：执行范围与后续流程](020-cli-image-input.md)。

### 9.4 多个入口、共用核心与两种出口

见 [020 §3.1：共用核心结构图与复用落点](020-cli-image-input.md#31-复用路径与最小修改面)。

### 9.5 参考 Codex 仍需补齐的功能与细节

见 [020 §5：场景、状态、副作用及验证方式](020-cli-image-input.md)。

### 9.6 四项具体规则：优先复用 Codex（2026-10-23）

见 [020 §2.1／§2.2：已确认的四项规则与持久化契约](020-cli-image-input.md#21-已确认规则)。
