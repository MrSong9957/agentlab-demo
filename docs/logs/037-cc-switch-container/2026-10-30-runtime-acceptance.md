# CC-Switch 运行验收证据

- 日期：2026-10-30；工作目录：容器内 `/workspace`（执行用户 `dev`）。
- 部署事件、验收入口与公共场景结果统一见 [036 运行验收日志](../036-node-pi-container/2026-10-30-runtime-acceptance.md)，不重复抄写。

## CC-Switch 专属结果

- 版本：`cc-switch --version` → 5.11.3，与该次构建解析的 GitHub latest Release v5.11.3（2026-10-22 发布）一致；实体为镜像内 `/usr/local/bin/cc-switch`（037 R1），非登录/登录/交互 shell 均可达（037 R2）。
- 数据库持久化（037 R3）：`/home/dev/.cc-switch` 与宿主 `.container-state/cc-switch` 跨路径标记一致；dev 可写读回；真实库（只读副本）`quick_check` ok。
- 全新初始化（037 §5 场景 4 容器内部分）：隔离 HOME 下真实 CLI 全新生成 `cc-switch.db` 并通过 `quick_check`；假标记在数据库重开后保留。
- 未执行登录、供应商切换、真实配置同步或代理服务（037 R4）；安装/刷新构建演示未在容器内执行，原因与影响评估见 036 日志「未执行与缺口」节（037 R5 的失败传播由 Dockerfile `&&` 串联与首轮真实构建成功佐证）。宿主侧补记（同日）：失败传播对照（B6）、非交互替换（B7）与刷新构建已在宿主执行完毕，见 [宿主侧引用](2026-10-30-host-side-runtime.md)。

## 结论

037/R1–R5 的容器内可验证项全部通过；验收状态由验收方在规格中判定。
