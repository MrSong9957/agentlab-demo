# loop 循环

## 1. 目标 
- 一个终端循环：读取用户输入 -> 打印回显（以`Agent：`为开头，输出干净文字，无乱码） -> 继续
- 输入 exit 或 Ctrl+D 退出
- 暂不接 LLM、不解析命令；历史仅内存内（本次会话上下键可回溯），不持久化
- 开场动画 和 结束提示

## 2. 边界风险
- 空输入：不打印空行，继续循环
- Ctrl+D：rustyline 返回 Eof，正常退出，不 panic
- Ctrl+C（分阶段统一说明）：启动/动画期间 readline 尚未开始，默认 SIGINT 直接终结进程（退出码 130，无结束提示）；输入就绪后 rustyline 捕获为 Interrupted，打印 `再见!` 正常退出（退出码 0）。两阶段均视为"直接退出"，符合原边界
- 非 UTF-8 输入不崩：rustyline 读到非法字节返回 Err，统一按正常退出处理，不回显乱码
- 检查动画期间提前输入。 用真实伪终端，在动画期间输入文字和方向键，检查是否丢键、乱码或出现字面转义序列。发现问题，先复现，再做最小修复。普通编辑测试应等输入界面就绪后发按键，提前输入另测，不能只靠延迟输入让测试通过。

## 3. 关键判断
- loop 循环先放 main 里跑通，暂不做其他多余事情
- 用 `rustyline` 而不是 `std::io::stdin()`：要支持方向键、历史、Ctrl+C 这类终端交互

## 4. 踩过的坑
- 逐行 `lines().unwrap()` 遇到非法 UTF-8 会返回 Err 导致崩溃，改为 `read_until(b'\n')` 按字节读入再 `from_utf8_lossy` 解码
- Ctrl+C 初版全生命周期靠默认 SIGINT 直接终结进程（实测退出码 130）；rustyline 化后仅启动/动画阶段仍如此，输入就绪后改为 Interrupted 正常退出——统一分阶段说明见第 2 节边界风险
- 转圈动画用 `\r` 原地刷新，输出接管道时帧会堆成一串乱码，用 `io::stdout().is_terminal()`（Rust 1.70+）判断，非终端跳过动画帧
- `from_utf8_lossy` 只保证不崩，但会把非法字节替换成 `�`（U+FFFD）原样回显出来；再按字符过滤掉 `\u{FFFD}` 才能做到回显干净无乱码
- rustyline 18 要求 edition 2024（Rust 1.85+），apt 的 rustc 1.75 编不过；项目未声明 MSRV，改为 rustup 安装 stable（rustc 1.99.0），Dockerfile 同步改为 rustup 装法，解除 unicode-segmentation 版本锁
- `--no-default-features` 会顺手关掉 `buffer-redux` 特性：rustyline 每次 `readline()` 重建读缓冲，`read()` 预读进来的字节在 accept 第一行后被丢弃——多行粘贴/提前排队输入从第 2 行起全丢（与动画无关，就绪后同样发生）。加回 `features = ["buffer-redux"]`（读缓冲跨 readline 复用）即修复
- `TERM=dumb` 或未设置时，rustyline 会静默退化为非交互模式：方向键变成字面文本、Ctrl+C 靠默认 SIGINT 杀进程；已在 compose 写入 `TERM: xterm-256color` 缺省值
- 动画期间（canonical 模式）提前输入会被行规程立即回显一次（出现在横幅上方），提示符就绪后 rustyline 再按序处理；动画期间 Ctrl+C 仍是默认 SIGINT（退出码 130），进入 readline 后才是 Interrupted 正常退出。（2026-10-15 注：行式契约不变；TUI 框架下按键单一职责契约（Ctrl+C 恒退出/Esc 取消/q 关页）见 008 §3-J10 修订与 012 §9）
- 新容器重建后 `cargo build` 链接失败（`Scrt1.o`/`crti.o`/`-lutil` 找不到）：apt 的 rustc 包硬依赖带进 `libc6-dev`，Dockerfile 改 rustup 装法后必须显式补 `libc6-dev`（`--no-install-recommends` 下 gcc 不会拉它）

## 5. 预测 -> 实测
- 输入 `hello` -> 期望打印 `Agent：hello`（带前缀，全角冒号） -> 实测（AI 填空）：打印 `Agent：hello`，退出码 0 ✅
- 连续回车 -> 期望无回显行 -> 实测：仅有 `你> ` 提示符，无 `Agent` 回显行，循环继续 ✅
- 方向键编辑（rustyline 新增） -> 输入 `helo` 按 ← 补 `l` 回车 -> 实测：提交 `hello`，回显 `Agent：hello` ✅
- 上下键历史（rustyline 新增） -> `hello` 回车后按 ↑ 再回车 -> 实测：再次回显 `Agent：hello`，内存历史生效 ✅
- Ctrl+D -> 期望退出码 0 -> 实测：rustyline 返回 Eof，打印 `再见!`，退出码 0 ✅
- Ctrl+C（更新） -> 期望直接退出 -> 实测：rustyline 捕获为 Interrupted，打印 `再见!`，退出码 0 ✅
- 开场动画（新增） -> 期望启动时播放 -> 实测：伪终端下转圈 10 帧约 0.6s 后出欢迎横幅；管道输出时自动跳过动画帧 ✅
- 结束提示（新增） -> 期望退出时有提示 -> 实测：`exit` / Ctrl+C / Ctrl+D 三条路径均打印 `再见!` 后退出码 0 ✅
- 干净文字（更新） -> 输入含非法字节如 `\xff` + `你好` -> 期望无 `�` -> 实测：rustyline 读错误按正常退出，输出无 `�`、不 panic；回显路径保留 `\u{FFFD}` 过滤兜底 ✅
- 收尾验收（rustc 1.99.0 + rustyline 18.0.1 + buffer-redux，全部重测）：
  - 左右键编辑（就绪）：`ab` ← `X` → 右移 → 插 `c` ← 补 `XY` -> 实测 `Agent：aXbXYc`，插入位置全部正确 ✅
  - 上下键历史（就绪）：`hello`/`world` 后 ↑↑ 提交得 `hello`，↑↑↓ 提交得 `world` ✅
  - Ctrl+C / Ctrl+D -> 实测：均打印 `再见!` 后退出码 0 ✅
  - 管道输入：`hello`/空行/`exit`/EOF -> 实测回显与退出码 0 正常，`exit` 后续输入不回显 ✅
  - 非法字节：管道与伪终端下均不 panic，无 `�` 输出，退出码 0 ✅
  - 提前输入（动画期间发键）：单行 `helo`+回车 -> 按序提交 `helo`，不丢键；`ab`+←+`cd` -> `acdb`，方向键被正确解析为编辑而非字面转义；多行 `a/b/c` 三行全部回显（buffer-redux 修复后）✅
  - 提前输入的剩余已知行为（非丢键/乱码）：动画期间输入会被行规程先回显一次（横幅上方出现一行），提示符就绪后 rustyline 按序处理；动画期间 Ctrl+C 为默认 SIGINT 直接退出（130），终端状态不残留 ✅（已记录，未修改）
- 新容器最终验收（宿主机重建 `agentlab-demo-dev` 后，2026-10-01）：
  - 环境：`rustc`/`cargo` 解析到 `/usr/local/cargo/bin`（rustup stable 1.99.0，非 1.75，无临时 export）；容器级（PID 1 environ，即 exec 会话继承值）`TERM=xterm-256color`、`CARGO_HOME=/home/dev/.cargo`（持久卷）、`RUSTUP_HOME=/usr/local/rustup` ✅
  - 编译：`cargo build --locked` 通过。首次重建暴露 libc6-dev 缺失（见踩坑），容器内补装 + Dockerfile 补齐后重建成功，以新产物验收 ✅
  - 功能（真实伪终端，逐项核对内容，非只看退出码）：左右键编辑 `aXbXYc` 精确；历史 ↑↑=`hello`、↑↑↓=`world`、单提示符 ↑↑↓=`y`（跨回车后游标复位为新提示符语义，非 bug）；Ctrl+C/Ctrl+D 均 `再见!`+0；提前单行/含方向键编辑 `acdb`/提前 `exit`/提前历史回溯 2 行/排队多行 3 行全部正确，无丢失、无重复处理、无字面转义 ✅
  - 管道与非法字节：管道回显/空行/exit/EOF 正常；非法字节管道与终端下均不 panic、无 `�`，退出码 0 ✅
  - 保留行为复核（与上文记录一致，未扩大修复）：动画期行规程预回显仍存在；Ctrl+C 动画期 130 / 就绪后 0 ✅
- 宿主机复核确认（2026-10-01，验收关闭）：新镜像已安装 `gcc libc6-dev`；新构建目录中依赖与主程序全量编译成功，旧构建缓存未掩盖问题。第 4 项验收全部通过，本规格收尾完成；动画期重复显示与 Ctrl+C 分阶段退出码（动画期 130 / 就绪后 0）按原约定保留，不再修改