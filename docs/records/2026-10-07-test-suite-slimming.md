# 测试套件精简：删除重复与低价值用例（2026-10-07）

## 设计依据

- 用户要求：优先保证关键业务（本项目无资金流，对应为凭证=资产、origin 授权=权限、目录/名单/写盘=数据）的测试深度，精简次要。
- 全量审查 92 个用例，其中 cargo 侧 90（catalog 31、llm_flow 49、display 7、api 3）+ pytest 2，按用户确认执行 A+B 档共 20 条：
  - A 档 12 条纯重复/无行为断言（如 `builtin_contains_deepseek_connection` 只断言代码常量；`same_cred_name_new_key_invalidates_query_list` 被 `identity_fields_each_invalidate` + 集成 `refresh_failure_after_key_change_excludes_old_list` 双重覆盖）。
  - B 档 8 条测第三方库内部（`pooled_connection_failure_post_not_resent` 断言 ureq 连接池行为）、环境敏感（`connect_timeout_rejected_cleanly` 依赖 10.255.255.1 不可路由）、容量边缘（8KB Key、2MB 输入、万条名单、100 页上限）。
- 合并 2 处：根层未知字段断言并入 `unknown_manual_field_rejected`；空值凭证断言并入 `credential_env_and_file`。
- 连带清理：catalog 测试辅助 `query_list`/`manual_with_key_ref` 失去使用者；`tests/common/mod.rs` 的 keep-alive mock 能力（`json_keepalive`、`keep_alive` 字段、连接复用循环）唯一使用者 `pooled_connection_failure_post_not_resent` 已删，一并移除；连接复用循环移除后 `method`/`path` 不再需要可变性，修复随之产生的 2 个 `unused_mut` 警告。
- C 档（公共目录可用性边缘、并发写、explicit_false 回归钉等）经讨论保留未动；display/api 单元测试为精炼白名单式安全测试，未动。

## 验证结果

- 用例数口径：cargo 侧 90 → 70（单元 41→32、集成 49→38）；pytest 2 条不变；总计 92 → 72（约 -22%）。
- 基线（改动前）：`cargo test --locked` 全绿（单元 41 + 集成 49 = 90）。
- 改动后：`cargo check` 通过；`cargo test --locked` 单元 32 + 集成 38 = 70 全绿，0 警告；`uv run pytest -q` 2 passed，合计 72/72 全绿。
- 每条删除的覆盖去向已逐条论证（重复被哪个保留用例覆盖），核心安全面（凭证目录限制、origin 授权、Key 泄露扫描、重定向、HTTPS 强制、换 Key 名单失效、原子写、显示过滤、管道 E2E）全部无损保留。

## 后续：合并凭证写盘辅助（同日，code-review Standards 轴发现）

### 合并方案

- 新建 `tests/common/cred_file_write.rs` 承载唯一实现 `write_cred_file_shared(tag, key)`（含 TCHAT_USER_DIR 前置断言防御）；`tests/common/mod.rs` 以 `pub use write_cred_file_shared as unique_cred_file` 保留集成测试入口（llm_flow 21 个调用点零改动）；`src/catalog.rs` 测试模块删除 `write_cred_file`，经 `include!("../tests/common/cred_file_write.rs")` 引入共享实现，2 个调用点改用共享名。crate 前缀差异（lib 内 `crate::` vs 集成测试 `terminal_chat::`）由两侧各一个一行 `cred_dir()` 薄壳解析。

### 为何选该方案

- 语义比对确认两份实现等价（路径=同一 `credential_dir()`、文件名/内容/返回格式逐字相同、均无显式权限位），唯一差异是 common 版多一层防误用断言，合并后两侧行为统一且更安全（catalog 侧调用点均在 UserDirGuard 之后，断言恒通过，行为不变）。
- 任务书备选"下沉到 src 侧 #[cfg(test)] 共享模块"不可行：`#[cfg(test)]` 项对集成测试目标不可见（test cfg 只在编译 lib 自身测试时开启），除非把辅助产品化（违反"禁止改 src 产品代码"约束）；"从 common 引入"在 lib 单元测试侧同样不可行（tests/ 目录不进入 lib 目标的编译图）。`include!` 文本引入不受目标边界限制，是同时满足"能编译 + 单一事实来源 + 零产品代码改动"的唯一方案。

### 验证结果

- `cargo check` 通过；`cargo test --locked` 单元 32 + 集成 38 = 70 全绿、0 警告；`uv run pytest -q` 2 passed，与合并前完全一致（无增删用例）。
- 旧名 `write_cred_file` 全仓库零残留；改动范围经 git 确认仅 src/catalog.rs 的测试模块、tests/common/mod.rs、新增 cred_file_write.rs 三个测试文件。
- 固定点提交：baseline `3786195`（92→72 全绿状态），合并提交 `857f39e`。
