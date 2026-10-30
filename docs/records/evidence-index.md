# 证据留存与原始快照索引

2026-10-19按批准方案整理原`docs/records/`的四个子目录。完整原件固定于Git提交`9e32a6347ae96d9ac2ff14d2655bdaf784d92fd9`；当前保留核心证据和摘要；2026-10-30 归档精简进一步移除工作树中的非 Markdown 证据文件，原件经 Git 历史按原路径取回。所有734份原件的路径、字节数、SHA-256、Git blob、处理理由及当前位置见逐文件清单。`snapshot_only`表示仅从当前工作树撤出，原件仍可取回；历史提交、manifest和空白原样保留，Git历史体积不会因此减少。

| 原目录 | 当前入口 | 完整原件 |
|---|---|---|
| `013-m7-provider-check/` | [M7核心证据](../testing/evidence/013-m7/README.md) | `9e32a63:docs/records/013-m7-provider-check/` |
| `security-20261005/` | [安全证据](../security/evidence/2026-10-05/README.md) | `9e32a63:docs/records/security-20261005/` |
| `v13-verification/` | [v13摘要](../testing/verification-history.md#v13) | `9e32a63:docs/records/v13-verification/` |
| `v14-verification/` | [v14摘要](../testing/verification-history.md#v14) | `9e32a63:docs/records/v14-verification/` |

按清单中的完整`original`路径读取原件，例如：

```sh
git show 9e32a63:docs/records/013-m7-provider-check/controller-fix/plan_a.py
git show 9e32a63:docs/records/v14-verification/cargo-test-workspace.log
```

以上命令只读取原件，不执行历史脚本。旧报告仍保留当时的运行命令、临时路径、预算和判定；它们的现行引用已指向核心附件或下面的快照条目。原M7 manifest内的文档指纹应对照`9e32a63`的文档，不能套用于之后更新过的当前文档。

<a id="m7-originals"></a>

## M7历史原件

<a id="archive-faf4d9adc193cba7"></a>

- 原路径：`docs/records/013-m7-provider-check/batch-367qctgq/manifest.json`；SHA-256：`e3b67ff1fb8f4b2e6a2cf8f1b3bb0d23e9de144e158acecfe699b61a37a52d3c`。

<a id="archive-b8f1312336bb57d6"></a>

- 原路径：`docs/records/013-m7-provider-check/batch-367qctgq/preservation.json`；SHA-256：`dfe9f6b1f94122c66889a53f0a801be2ac6a53ba10f5086b631cf3cd6e4e378a`。

<a id="archive-4edf91a35bf4ee78"></a>

- 原路径：`docs/records/013-m7-provider-check/batch-summary.json`；SHA-256：`8c94070eaea0025762722f8debbcb05a5a415dbff9ae95994ea48e4dd51f7153`。

<a id="archive-f91f83ba9d45722e"></a>

- 原路径：`docs/records/013-m7-provider-check/cfix-live-2026-10-19/audit/readiness-before-prepare.json`；SHA-256：`2894e5033ba86caff0170e01bce6a4d07ed24c1deb80de87a5e6a5a659ccf135`。

<a id="archive-a50e34a735b2ce5f"></a>

- 原路径：`docs/records/013-m7-provider-check/cfix-live-2026-10-19/manifest.json`；SHA-256：`832b538b909cabf9cc8aea30d50d236e78b2f638385d0e2979d0593da5d593d0`。

<a id="archive-9e1a4d4f6f2e4c13"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/controller_proofs.py`；SHA-256：`0ac26c5a1fba16e3914e5635f02b2bd01ebea8b8fdd2991e732b07e367f4c174`。

<a id="archive-203296b693c3f7f5"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/failed-to-fixed-controller.diff`；SHA-256：`6494045b1667b8c2d170052905c9657dc432fae221d8c72dc218f7bc0c7279fb`。

<a id="archive-f20e28736a0010bb"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/harness.rs`；SHA-256：`932e8ed182ce1b047dcabaffbc149e61962ae34dda89191d6493deb527a2b41c`。

<a id="archive-50cd14d331bfa370"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/manifest.json`；SHA-256：`6cb14b3089d2cdda3b9661fa4700d1081c731ecde598e2eef81edf923554bd8b`。

<a id="archive-3418de07b7baf583"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/plan_a.py`；SHA-256：`e5f0d968fa18b59623d8a7bb556f361a3151ce2b60b0419bf2f8b09f9c4b3f89`。

<a id="archive-bb4b1bee4382eb65"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/preservation.json`；SHA-256：`40b5673a1c8e6123797396b763a066048693132db3b9c10449b4d66f3bc1b91e`。

<a id="archive-0ecce0971c43aba5"></a>

- 原路径：`docs/records/013-m7-provider-check/controller-fix/verify_controller.py`；SHA-256：`1c3c7546690b4f398d13fa1460774dd5e514770d66374e86de45030ed0b9980f`。

<a id="archive-e4c5e48f35f1d3ef"></a>

- 原路径：`docs/records/013-m7-provider-check/evidence-manifest.json`；SHA-256：`d1cadb8bd6cadc60c4a6845389e0d9432d556b5f965f74036de1ab1909aa7184`。

<a id="archive-a52ac92df34cd324"></a>

- 原路径：`docs/records/013-m7-provider-check/failed-batch-2026-10-19/evidence-manifest.json`；SHA-256：`d1cadb8bd6cadc60c4a6845389e0d9432d556b5f965f74036de1ab1909aa7184`。

<a id="archive-695335d98badc476"></a>

- 原路径：`docs/records/013-m7-provider-check/harness.rs`；SHA-256：`148626298729769ad4e0461503a03ec951d6dff66916533d5ac3af803d21f82b`。

<a id="archive-88e6e429e92362bc"></a>

- 原路径：`docs/records/013-m7-provider-check/ledger.json`；SHA-256：`27a98881e611357100ed163913d3ac5eb243a4b6c5e32f9a45e99d0e1410ca37`。

<a id="archive-1bf0109590553a79"></a>

- 原路径：`docs/records/013-m7-provider-check/local-diagnosis.json`；SHA-256：`727dad30016955e9c3261b3712b6301f18517bf82ac353a2291cea024eab4d41`。

<a id="archive-4c52c66e3cd0c280"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-checks.json`；SHA-256：`b1c83210aeffd341f33cb03bd94616127cfa3a0acb73172267fcef45e99f50a8`。

<a id="archive-048a929324068762"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-fix/failure-invocation.json`；SHA-256：`cf2f3274f1a18b4f385b981b88b9671ff38dd7d8ae0c1bb552a281989bf99798`。

<a id="archive-3f1049d27ce43c67"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-fix/frozen-to-fixed.diff`；SHA-256：`4a81415725ce545018e3104a4bb7358d6a311457e3a8d527bef74fb29116ceea`。

<a id="archive-cade8551ca20442a"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-fix/manifest.json`；SHA-256：`59b54c6762beeeef38a19ef4227cb473c76f0e5fb25d28e8549d95971992e0bf`。

<a id="archive-41662447871798ac"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-fix/preservation.json`；SHA-256：`bddd6232d4608ab81a13a2606461c3baddb4b93be6d4b0b64de81d32c575074e`。

<a id="archive-e4739075b6f520e3"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-fix/red-harness.rs`；SHA-256：`3e28046561b4f47ccbc20545034aa562c79026f98f01281a2a9a03084e037ab8`。

<a id="archive-2b1698c3cc7e45e8"></a>

- 原路径：`docs/records/013-m7-provider-check/offline-review-367qctgq/offline_replay_review.py`；SHA-256：`d1413de223a371943624474647d2aeb9e1461d7bcbb0c3b85f13f041e90685d6`。

<a id="archive-f44a0dc95c784c8c"></a>

- 原路径：`docs/records/013-m7-provider-check/offline_cli_checks.py`；SHA-256：`e5331123be08f319b75a734b786ce4a8fe4ce9a837d6789e65bbb454d789e14b`。

<a id="archive-458edbc40835ac12"></a>

- 原路径：`docs/records/013-m7-provider-check/path-prefix-repro.rs`；SHA-256：`8536a2bfd1b875661f4aac885085c6b988cd5c310e5c6fb691f73c42f6992ea7`。

<a id="archive-86609132ce2d6e3b"></a>

- 原路径：`docs/records/013-m7-provider-check/phase1-process.json`；SHA-256：`59cce5efb301a528021a7a70fe41d8c31f53948748ea87d83580134a0955ef32`。

<a id="archive-76dcb5cf3d06c9df"></a>

- 原路径：`docs/records/013-m7-provider-check/plan-a-2026-10-19-stopped/failed-dispatch.py`；SHA-256：`dfdeefac2e5e682ca86b38aa333e770922ea18b0d5f389be541820975b091322`。

<a id="archive-98b4a355319598e5"></a>

- 原路径：`docs/records/013-m7-provider-check/plan-a-2026-10-19-stopped/manifest.json`；SHA-256：`c71fab046749a93db3aab00564a5b1f45c74983f30286dca4506592473d57213`。

<a id="archive-de8e179eeedf26e0"></a>

- 原路径：`docs/records/013-m7-provider-check/plan-a-2026-10-19-stopped/preservation.json`；SHA-256：`1bf1f1e47ac4449d746dbe2a5fd9b15ee90c2677f2d3874bf98aa37dee883e4b`。

<a id="archive-05ddba711f6538f3"></a>

- 原路径：`docs/records/013-m7-provider-check/plan-a-2026-10-19-stopped/protection-before-results.json`；SHA-256：`8815efe18c8bdca24ec5981be80d6b499d65a99f661fe785d33690bd17f66a75`。

<a id="archive-2447d6bf9069f219"></a>

- 原路径：`docs/records/013-m7-provider-check/preservation.json`；SHA-256：`cb32bf64352f51e2cc3ccaced8a537998543b8e2a236d67a5269f2123e0bdaa5`。

<a id="archive-e9425e8a6f71a2c4"></a>

- 原路径：`docs/records/013-m7-provider-check/run_batch.py`；SHA-256：`f27ebcb6c09817afbe4110bae1a2a62e311309f2cd53586128aeea49747f44a3`。

<a id="archive-64e3933085480a38"></a>

- 原路径：`docs/records/013-m7-provider-check/v2-fixture/harness-v1-frozen.rs`；SHA-256：`a971b2faf1e9c838623e3a84513264da1d426c7e463af914f5b735a389d594d3`。

<a id="archive-720b5c4b318a4e29"></a>

- 原路径：`docs/records/013-m7-provider-check/v2-fixture/manifest.json`；SHA-256：`ef76704f899eec59ee8982e9967e101d9da3d11682123a38c3b65d0755a0973d`。

<a id="archive-e9459cb1948398db"></a>

- 原路径：`docs/records/013-m7-provider-check/v2-fixture/v2-verification.json`；SHA-256：`0e659081778ccc865880ddffb2ceafd3ed57801f91237efb4d26b82f19f213c0`。

<a id="archive-72177654b2aa9af3"></a>

- 原路径：`docs/records/013-m7-provider-check/v2-fixture/verify_v2_fixture.py`；SHA-256：`350b4687e66447b506edbabe610e9aa8dc03982a02c7b2fae60fbf5117919ba1`。

<a id="security-originals"></a>

## 安全历史原件

<a id="archive-651e36d7f2d538f1"></a>

- 原路径：`docs/records/security-20261005/attack.py`；SHA-256：`57a1e4a622181f1dd406a40e7a301cf51f4cd6266b34cb1a2cc323eefc3ef4a7`。

<a id="verification-originals"></a>

## v13／v14完整日志与说明

<a id="archive-0797ef9a979a2045"></a>

- 原路径：`docs/records/v13-verification/README.md`；SHA-256：`6ebffcdc6a7be22b87a7c051742946ad859a7e60cd782a99d318c8e089a011a6`。

<a id="archive-a0cf2921f5274f11"></a>

- 原路径：`docs/records/v13-verification/cargo-check.log`；SHA-256：`dfc4a7207a7ed50f1558a072afe941bf446e233037671503ab5973ca3b510149`。

<a id="archive-c696d2a63ab13661"></a>

- 原路径：`docs/records/v13-verification/cargo-test-e2e.log`；SHA-256：`c89bc7d99e32768c278fa4dc2b99a7e3520329099c988ce1eadfa66c4a851631`。

<a id="archive-3112d2460a791c49"></a>

- 原路径：`docs/records/v13-verification/cargo-test-workspace.log`；SHA-256：`fa1446c940f33d8e5c617468e5ad48672cf0f85f6fcb15cbe7e4a8eb1e1d344e`。

<a id="archive-0941c08a1592bd3e"></a>

- 原路径：`docs/records/v14-verification/README.md`；SHA-256：`0f5987ce0661c63bae99b6aff4ee8ace60ef4953683ef4343a7d851106825ea6`。

<a id="archive-588c7876011d9eb8"></a>

- 原路径：`docs/records/v14-verification/cargo-check.log`；SHA-256：`e67f5c84077dbb3a0f8347acd2e30fa95d3287166cc5f7c0dd5d3493d5f13319`。

<a id="archive-cf13c1832efee9fa"></a>

- 原路径：`docs/records/v14-verification/cargo-test-e2e.log`；SHA-256：`b0b6c90abc6e03ae4e87a82ec9e8b6eebf9fa50282b4b033d6c1930e2e9e73f9`。

<a id="archive-62f014dc4d16d64e"></a>

- 原路径：`docs/records/v14-verification/cargo-test-workspace.log`；SHA-256：`136fd2db0b9e856bc4e754686c55fa3c53e7778c20aed8f5709a9f4258125df0`。

## 018 同步问询（2026-10-22）

[实施与门禁记录](2026-10-22-018-user-input-implementation.md)汇总固定复用来源、CLI/PTY 验收与全量验证；五类关键回归有效性保留实际可逆破坏补丁、目标失败与恢复哈希。本条为新增证据，不改变上方历史整理的 734 份原件清单。

阶段 B 的[固定源码调查与最小契约草案](../specs/018-user-input.md#8-阶段-b最小异步问询契约草案)记录发布/回答分路、本地 V1 读取保存探针、待确认兼容取舍及后续验收。该调查不是异步功能通过证据；未修改程序或注册工具，B 验收均待验证。

2026-10-23 阶段 B 实施（契约确认后）：[实施与验证记录](2026-10-23-018b-implementation.md)汇总 TUI 七工具/F6/等待/批末接入/来源标记的实现、14 项验收映射、安全门禁新增三项与 lint 边界；六类关键回归有效性（误当聊天/批中插答/未等即做/旧答串题/取消不醒/恢复错显）保留可逆破坏补丁、失败摘录与恢复哈希。全量 3171/0/3（相对阶段 A 交付基线 3142/0/3 净增 29）；2026-10-23 批准交付本地 main（任务提交 `49060bf`，本地任务分支已删除，未推送）。

2026-10-23 [交付后复核](2026-10-23-018-user-input-audit.md)：既有全量回归通过，但三个独立探针复现等待批次、协议错误保留答案、已答 ID 再等待的行为缺口；另记录两项测试/证据规范问题。问题尚未修复，不把先前全绿结论等同于无缺陷。

2026-10-23 [审计修复轮](2026-10-23-018-user-input-audit.md)（分支 `fix/018-user-input-audit`）：五项全部闭环——三条探针移入默认回归（先 RED 后 GREEN）、批末校验/协议错误收尾/已答 ID 就绪三项修复、测试临时目录改隔离夹具；M3/M6 哈希按 git 核实修正（未提交中间态不可取证，字段更正为重录真实原始哈希并注明历史），并在修复候选上重放九项变异（M1-M6＋A1-A3）全部检出复绿，见 018b-mutations.json `audit_fix_replay` 节。
