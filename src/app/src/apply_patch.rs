//! 文本补丁修改（apply_patch，004 规格）：完整解析 Codex 补丁格式后，按顺序
//! 新增、局部修改或删除 UTF-8 文本文件，返回实际变更清单。
//! 参考 openai/codex 提交 2fdf047c 的 apply-patch（parser / streaming_parser /
//! file_update / text_file / seek_sequence）。
//!
//! MVP 约束（规格第 3 节）：
//! - 支持 Add / Update / Delete；`*** Update File` 块可带 `*** Move to: 目标`
//!   （029：移动/重命名＝先排他创建目标并完整写入、复核源后删除源；一个成功
//!   Move 记两条＝目标 add＋源 delete）。不支持二进制文件。
//! - 新增不得覆盖已有文件（`create_new` 原子保证，含悬空符号链接）。
//! - 先完整解析，再按补丁顺序逐文件校验并执行；失败即停止，不回滚此前修改。
//! - 更新先算出完整新内容再写；匹配失败不改当前文件。
//! - 行尾：未改行保留原换行符；插入/替换行用原文件首个换行风格（无则 LF）；
//!   新增文件用 LF；更新后补齐末尾换行。
//!
//! 与 Codex 参考实现的取舍：
//! - 行匹配保留四级容差（精确 → 忽略行尾空白 → 忽略两端空白 → 忽略两端空白
//!   ＋Unicode 标点/空格映射，029 移植上游 seek_sequence 最后一轮；映射仅用于
//!   定位比较，不改写源文件缓冲）。
//! - 解析错误与校验失败（未发生写入）`state_known=true`；写入/删除失败的
//!   变更操作可能已部分修改目标（坑003），`state_known=false` 须重读。
//! - 按引用语法 `add_line+` 要求 Add 块至少一行 `+` 内容（Codex 解析器未强制）。

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};

// ---- JSON 工具边界 ----

/// apply_patch 工具入参：只接受 patch 字段，多余字段视为非法。
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyPatchArgs {
    pub patch: String,
}

impl ApplyPatchArgs {
    /// 从工具边界的 JSON 值解析并校验；任何非法输入返回错误说明，不触碰文件。
    pub fn from_value(v: &serde_json::Value) -> Result<Self, String> {
        let Some(obj) = v.as_object() else {
            return Err("参数必须是对象".into());
        };
        for key in obj.keys() {
            if key != "patch" {
                return Err(format!("未知参数：{key}"));
            }
        }
        match obj.get("patch") {
            None | Some(serde_json::Value::Null) => Err("缺少必填参数 patch".into()),
            Some(p) => {
                let s = p.as_str().ok_or("patch 必须是字符串")?;
                if s.len() > MAX_PATCH_BYTES {
                    return Err(format!(
                        "patch 文本 {len} 字节超过 {MAX_PATCH_BYTES} 字节（1 MiB）上限",
                        len = s.len()
                    ));
                }
                Ok(Self { patch: s.to_string() })
            }
        }
    }
}

/// 文件动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Add,
    Update,
    Delete,
}

/// 一条确认生效的变更。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// 实际目标的绝对路径。
    pub path: String,
    pub action: Action,
}

/// 执行结果：四个字段始终存在。
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplyPatchResult {
    pub ok: bool,
    /// 按执行顺序列出确认生效的变更。
    pub changed_files: Vec<ChangedFile>,
    /// 变更清单是否完整可信；写入/删除失败的目标可能被部分修改，须重读。
    pub state_known: bool,
    /// 成功时为 null；失败时为包含原因和可确定目标路径的字符串。
    pub error: Option<String>,
}

impl ApplyPatchResult {
    fn error(msg: String, changed: Vec<ChangedFile>, state_known: bool) -> Self {
        Self { ok: false, changed_files: changed, state_known, error: Some(msg) }
    }
}

/// 工具入口：解析校验入参，完整解析补丁后按顺序应用。
pub fn apply_patch(input: &serde_json::Value) -> ApplyPatchResult {
    apply_patch_with_display(input).0
}

/// 033：沙箱模式（`TCHAT_SANDBOX`）下以 bwrap 包装的本二进制 `--internal-apply`
/// 子进程应用补丁——补丁文本经 stdin 传入，读、判型、写全部发生在沙箱子进程
/// （复用进程内同套边界与 /proc/self/fd 同 inode 语义）；结果
/// (ApplyPatchResult, 展示变更) JSON 经 stdout 返回。不经 shell（execve 数组
/// 直传）。沙箱不可用＝结果内显式错误，不静默回退为进程内执行。
pub fn apply_patch_sandboxed(
    input: &serde_json::Value,
) -> (ApplyPatchResult, Vec<crate::prompt::PatchChange>) {
    // 生产入口＝当前二进制。集成测试进程的 current_exe 是测试 harness，
    // 由测试经 [`apply_patch_sandboxed_with_exe`] 传 CARGO_BIN_EXE 解析。
    match std::env::current_exe() {
        Ok(exe) => apply_patch_sandboxed_with_exe(&exe, input),
        Err(e) => (
            ApplyPatchResult::error(format!("定位自身二进制失败：{e}"), Vec::new(), false),
            Vec::new(),
        ),
    }
}

/// [`apply_patch_sandboxed`] 的显式二进制变体（测试注入 CARGO_BIN_EXE）。
pub fn apply_patch_sandboxed_with_exe(
    exe: &std::path::Path,
    input: &serde_json::Value,
) -> (ApplyPatchResult, Vec<crate::prompt::PatchChange>) {
    let err =
        |msg: String, state_known: bool| (ApplyPatchResult::error(msg, Vec::new(), state_known), Vec::new());
    let args = match ApplyPatchArgs::from_value(input) {
        Ok(a) => a,
        Err(msg) => return err(msg, true),
    };
    // 策略锚定主进程 cwd（G1 修复，sandbox 内部取用）；补丁路径的解析基准
    // 与进程内执行一致＝主进程 cwd。
    let (program, sandbox_args, child_env) = match crate::sandbox::tool_launch_argv(vec![
        exe.to_string_lossy().into_owned(),
        "--internal-apply".to_string(),
    ]) {
        Ok(Some(t)) => t,
        Ok(None) => return err("沙箱模式未启用（TCHAT_SANDBOX 未设置）".into(), false),
        Err(e) => return err(format!("沙箱不可用：{e}"), false),
    };
    use std::io::Write;
    use std::process::Stdio;
    let mut spawn_cmd = std::process::Command::new(program);
    spawn_cmd.args(sandbox_args);
    // 033-B2：同 exec 面——继承 fd ≥3 标记 CLOEXEC（execve 时关闭）；仅沙箱
    // 分支。pre_exec 为 unix API；非 unix 下 tool_launch_argv 恒 None，不会
    // 到达此分支（cfg 门控仅为可编译性）。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            spawn_cmd.pre_exec(crate::sandbox::close_inherited_fds_mark_cloexec);
        }
    }
    let child = spawn_cmd
        .env_clear() // 033：凭据环境不进补丁子进程（027 R1 环境面防线保留）
        .envs(child_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return err(format!("沙箱执行启动失败：{e}"), false),
    };
    if let Err(e) = child.stdin.take().unwrap().write_all(args.patch.as_bytes()) {
        return err(format!("补丁传输失败：{e}"), false);
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return err(format!("沙箱执行等待失败：{e}"), false),
    };
    if !out.status.success() {
        let all = String::from_utf8_lossy(&out.stderr);
        let skip = all.chars().count().saturating_sub(200);
        let tail: String = all.chars().skip(skip).collect();
        return err(format!("沙箱执行失败（exit {:?}）：{tail}", out.status.code()), false);
    }
    match serde_json::from_slice::<(
        ApplyPatchResult,
        Vec<crate::prompt::PatchChange>,
    )>(&out.stdout) {
        Ok(parsed) => parsed,
        Err(e) => err(format!("沙箱执行结果解析失败：{e}"), false),
    }
}

/// 033：`--internal-apply` 入口（仅由 [`apply_patch_sandboxed`] 经 bwrap 包装
/// 运行；不属于模型工具契约，沿用 027 引入的子进程模式）。stdin 读补丁 →
/// 进程内同一 run() → (结果, 变更) JSON 写 stdout。退出码：0＝完成（结果看
/// JSON）、2＝协议失败。
pub fn run_internal_apply() -> i32 {
    use std::io::Read;
    // 流式限量：至多读 MAX_PATCH_BYTES+1 字节，超限即拒（先于整段入内存/转字符串）
    let mut bytes = Vec::new();
    let mut limited = std::io::stdin().take(MAX_PATCH_BYTES as u64 + 1);
    if limited.read_to_end(&mut bytes).is_err() {
        eprintln!("internal-apply：读取补丁失败");
        return 2;
    }
    if bytes.len() > MAX_PATCH_BYTES {
        eprintln!("internal-apply：补丁超过 {} 字节上限", MAX_PATCH_BYTES);
        return 2;
    }
    let patch = match String::from_utf8(bytes) {
        Ok(p) => p,
        Err(_) => {
            eprintln!("internal-apply：补丁不是合法 UTF-8");
            return 2;
        }
    };
    let (result, changes) = run(&patch);
    match serde_json::to_writer(std::io::stdout(), &(result, changes)) {
        Ok(()) => {
            println!();
            0
        }
        Err(_) => 2,
    }
}

/// 012：应用补丁并返回**展示用逐文件差异**（与结果并行返回；不混入结果 JSON——
/// 模型契约不变）。差异全部来自执行时的真实前后内容：Add＝新全文、Delete＝旧全文
///（读入超限/失败时 `None`，展示层明确降级）、Update＝对真实前后缓冲生成的统一
/// diff；不从补丁文本反推行号。执行中途失败时返回已生效前缀的差异。
pub fn apply_patch_with_display(
    input: &serde_json::Value,
) -> (ApplyPatchResult, Vec<crate::prompt::PatchChange>) {
    let args = match ApplyPatchArgs::from_value(input) {
        Ok(a) => a,
        Err(msg) => {
            return (ApplyPatchResult::error(msg, Vec::new(), true), Vec::new());
        }
    };
    run(&args.patch)
}

/// 固定边界（2026-10-10 安全修复 N5）：补丁 ≤1 MiB；单文件读入与生成内容
/// ≤4 MiB 且 ≤100000 逻辑行（CRLF 算一行；非空且无末尾换行的尾段算一行）。
pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FILE_LINES: usize = 100_000;

/// 行切分原语（零分配）：逐段产出 `(行文本, 行尾)`——\r、\r\n、\n 均为行尾，
/// 非空尾段计一行，空串不产出。[`SourceFile::parse`] 与 [`logical_lines`] 共用
/// 本原语：行数门与解析的行定义由**构造**保证同源（安全修复 F-1；审查 #10
/// 进一步消除计数路径的全量物化分配）。
fn line_ranges(s: &str) -> impl Iterator<Item = (&str, Option<LineEnding>)> {
    let bytes = s.as_bytes();
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos >= s.len() {
            return None;
        }
        let start = pos;
        let mut i = pos;
        let split = loop {
            if i >= s.len() {
                break None;
            }
            match bytes[i] {
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                    break Some((LineEnding::CrLf, 2));
                }
                b'\r' => break Some((LineEnding::Cr, 1)),
                b'\n' => break Some((LineEnding::Lf, 1)),
                _ => i += 1,
            }
        };
        match split {
            Some((ending, len)) => {
                pos = i + len;
                Some((&s[start..i], Some(ending)))
            }
            None => {
                pos = s.len();
                Some((&s[start..], None))
            }
        }
    })
}

/// 逻辑行计数：零分配（仅迭代 [`line_ranges`]）。安全修复 F-1（2026-10-28 攻防
/// 报告）：此前用 `str::lines()`（仅按 \n 切分），lone-CR 文件被漏计成「1 行」
/// 通过读入门、实际展开百万级行——行数门与解析行定义必须统一，不得再分叉。
fn logical_lines(s: &str) -> usize {
    line_ranges(s).count()
}

fn run(patch_text: &str) -> (ApplyPatchResult, Vec<crate::prompt::PatchChange>) {
    let hunks = match parse_patch(patch_text) {
        Ok(h) => h,
        Err(msg) => {
            return (ApplyPatchResult::error(msg, Vec::new(), true), Vec::new());
        }
    };
    if hunks.is_empty() {
        return (
            ApplyPatchResult::error("补丁不包含任何文件操作（空补丁）".into(), Vec::new(), true),
            Vec::new(),
        );
    }
    let base = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            return (
                ApplyPatchResult::error(
                    format!("无法确定会话当前目录：{e}"),
                    Vec::new(),
                    true,
                ),
                Vec::new(),
            );
        }
    };
    let mut changed: Vec<ChangedFile> = Vec::new();
    let mut display: Vec<crate::prompt::PatchChange> = Vec::new();
    for hunk in &hunks {
        // 029：执行器按实际完成的文件操作即时累积清单与展示（Move 一块可产出
        // 多条，部分成功也保留已完成部分）。
        if let Err(Failure { msg, state_known }) =
            apply_hunk(hunk, &base, &mut changed, &mut display)
        {
            return (ApplyPatchResult::error(msg, changed, state_known), display);
        }
    }
    (
        ApplyPatchResult { ok: true, changed_files: changed, state_known: true, error: None },
        display,
    )
}

// ---- 补丁解析（Codex streaming parser 的整段移植）----

const BEGIN_MARKER: &str = "*** Begin Patch";
const END_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// 解析出的补丁块，按出现顺序排列。
#[derive(Debug, PartialEq, Clone)]
enum Hunk {
    Add {
        path: PathBuf,
        /// 每行以 LF 结尾（新增文件用 LF）。
        content: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        /// 029：`*** Move to:` 目标路径；仅允许紧接 Update 头，每块最多一次。
        move_to: Option<PathBuf>,
        chunks: Vec<UpdateChunk>,
    },
}

/// Update 块中的一个变更块：由可选 `@@` 定位行引入。
#[derive(Debug, Default, PartialEq, Clone)]
struct UpdateChunk {
    /// 单行定位上下文（通常是类名/函数名），须出现在上一个块匹配位置之后。
    change_context: Option<String>,
    /// 被替换的连续行；空表示纯插入（追加到文件末尾）。
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    /// 上下文行在 old_lines / new_lines 中的对应下标，用于保留原始行尾。
    context_line_indices: Vec<(usize, usize)>,
    /// 为真时 old_lines 须匹配在文件末尾（容差锚定）。
    is_end_of_file: bool,
}

impl UpdateChunk {
    /// 上下文行同时进入两侧并记录下标，与推断相等的行区分开。
    fn push_context_line(&mut self, line: String) {
        self.context_line_indices.push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Mode {
    NotStarted,
    Started,
    Add,
    Delete,
    Update,
    Ended,
}

struct Parser {
    hunks: Vec<Hunk>,
    mode: Mode,
}

impl Parser {
    fn process_line(&mut self, raw: &str, no: usize) -> Result<(), String> {
        let trimmed = raw.trim();
        match self.mode {
            Mode::NotStarted => {
                if trimmed == BEGIN_MARKER {
                    self.mode = Mode::Started;
                    Ok(())
                } else {
                    Err(format!("第 {no} 行：补丁第一行必须是「{BEGIN_MARKER}」"))
                }
            }
            Mode::Started => {
                if self.handle_headers(trimmed, no)? {
                    Ok(())
                } else {
                    Err(format!(
                        "第 {no} 行：'{trimmed}' 不是有效的块头，有效块头：\
                         *** Add File: <路径>、*** Delete File: <路径>、*** Update File: <路径>"
                    ))
                }
            }
            Mode::Add => {
                if self.handle_headers(trimmed, no)? {
                    return Ok(());
                }
                if let Some(line) = raw.strip_prefix('+') {
                    if let Some(Hunk::Add { content, .. }) = self.hunks.last_mut() {
                        content.push_str(line);
                        content.push('\n');
                        return Ok(());
                    }
                    unreachable!("Add 模式下末尾块必为 Add");
                }
                Err(format!("第 {no} 行：Add File 块中的行必须以 + 开头：'{trimmed}'"))
            }
            Mode::Delete => {
                if self.handle_headers(trimmed, no)? {
                    Ok(())
                } else {
                    Err(format!(
                        "第 {no} 行：'{trimmed}' 不是有效的块头（Delete File 块不包含内容行）"
                    ))
                }
            }
            // Update 模式下块头只按行尾裁剪匹配：带前导空格的 marker 视为上下文行
            Mode::Update => {
                let end_trimmed = raw.trim_end();
                if self.handle_headers(end_trimmed, no)? {
                    return Ok(());
                }
                self.update_lines(raw, end_trimmed, no)
            }
            Mode::Ended => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(format!("补丁在「{END_MARKER}」之后不得有多余内容"))
                }
            }
        }
    }

    /// 识别 End Patch 与三类块头；识别到返回 true。参数行已按调用方约定裁剪
    /// （顶层/Add/Delete 传 trim()，Update 传 trim_end()）。离开块时做完整性校验。
    fn handle_headers(&mut self, line: &str, no: usize) -> Result<bool, String> {
        let is_end = line == END_MARKER;
        let add = line.strip_prefix(ADD_FILE_MARKER);
        let delete = line.strip_prefix(DELETE_FILE_MARKER);
        let update = line.strip_prefix(UPDATE_FILE_MARKER);
        if !is_end && add.is_none() && delete.is_none() && update.is_none() {
            return Ok(false);
        }
        match self.mode {
            Mode::Add => {
                if let Some(Hunk::Add { path, content }) = self.hunks.last() {
                    if content.is_empty() {
                        return Err(format!(
                            "第 {no} 行：Add File 块（路径 {p}）至少需要一行以 + 开头的内容",
                            p = path.display()
                        ));
                    }
                }
            }
            Mode::Update => {
                if let Some(Hunk::Update { path, move_to, chunks, .. }) = self.hunks.last() {
                    // 029：带 Move 的空 Update＝纯移动，合法；无 Move 且无修改块仍为空块
                    if chunks.is_empty() && move_to.is_none() {
                        return Err(format!(
                            "第 {no} 行：Update File 块（路径 {p}）为空",
                            p = path.display()
                        ));
                    }
                    if chunks.last().is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
                    {
                        return if is_end {
                            Err(format!("第 {no} 行：更新块不包含任何变更行"))
                        } else {
                            Err(unexpected_update_line(no, line))
                        };
                    }
                }
            }
            _ => {}
        }
        if is_end {
            self.mode = Mode::Ended;
            return Ok(true);
        }
        if let Some(p) = add {
            self.hunks.push(Hunk::Add { path: check_path(p, no)?, content: String::new() });
            self.mode = Mode::Add;
            return Ok(true);
        }
        if let Some(p) = delete {
            self.hunks.push(Hunk::Delete { path: check_path(p, no)? });
            self.mode = Mode::Delete;
            return Ok(true);
        }
        if let Some(p) = update {
            self.hunks.push(Hunk::Update {
                path: check_path(p, no)?,
                move_to: None,
                chunks: Vec::new(),
            });
            self.mode = Mode::Update;
        }
        Ok(true)
    }

    /// Update 模式下块头之外的行：@@ 定位、End of File、上下文/新增/删除行。
    fn update_lines(&mut self, raw: &str, end_trimmed: &str, no: usize) -> Result<(), String> {
        let Some(Hunk::Update { move_to, chunks, .. }) = self.hunks.last_mut() else {
            unreachable!("Update 模式下末尾块必为 Update");
        };

        // End of File 标记之后：只允许空行或新的 @@ 块
        if chunks.last().is_some_and(|c| c.is_end_of_file) {
            if end_trimmed.is_empty() {
                return Ok(());
            }
            if end_trimmed != EMPTY_CHANGE_CONTEXT_MARKER
                && !end_trimmed.starts_with(CHANGE_CONTEXT_MARKER)
            {
                return Err(expected_at_at(no, raw));
            }
        }

        // 029：Move to 只允许紧接 Update 头（chunks 为空＝尚未开始任何修改块）；
        // 重复与空目标在完整解析阶段拒绝。修改行之后出现时落入下方意外行错误。
        if chunks.is_empty() {
            if let Some(dest) = end_trimmed.strip_prefix(MOVE_TO_MARKER) {
                if move_to.is_some() {
                    return Err(format!(
                        "第 {no} 行：*** Move to 在同一 Update 块中只能出现一次"
                    ));
                }
                *move_to = Some(check_path(dest, no)?);
                return Ok(());
            }
            if end_trimmed == MOVE_TO_MARKER.trim_end() {
                return Err(format!("第 {no} 行：*** Move to 缺少目标路径"));
            }
        }

        // 连续 @@：上一个 @@ 块仍为空
        if (end_trimmed == EMPTY_CHANGE_CONTEXT_MARKER
            || end_trimmed.starts_with(CHANGE_CONTEXT_MARKER))
            && chunks.last().is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
        {
            return Err(unexpected_update_line(no, raw));
        }

        if end_trimmed == EMPTY_CHANGE_CONTEXT_MARKER {
            chunks.push(UpdateChunk::default());
            return Ok(());
        }
        if let Some(ctx) = end_trimmed.strip_prefix(CHANGE_CONTEXT_MARKER) {
            chunks.push(UpdateChunk {
                change_context: Some(ctx.to_string()),
                ..UpdateChunk::default()
            });
            return Ok(());
        }
        if end_trimmed == EOF_MARKER {
            if chunks.last().is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty()) {
                return Err(format!("第 {no} 行：更新块不包含任何变更行"));
            }
            if let Some(c) = chunks.last_mut() {
                c.is_end_of_file = true;
            }
            return Ok(());
        }
        if raw.is_empty() {
            // 空行按空上下文行处理（Codex 同样宽容）
            current_chunk(chunks).push_context_line(String::new());
            return Ok(());
        }
        if let Some(line) = raw.strip_prefix(' ') {
            current_chunk(chunks).push_context_line(line.to_string());
            return Ok(());
        }
        if let Some(line) = raw.strip_prefix('+') {
            current_chunk(chunks).new_lines.push(line.to_string());
            return Ok(());
        }
        if let Some(line) = raw.strip_prefix('-') {
            current_chunk(chunks).old_lines.push(line.to_string());
            return Ok(());
        }
        if chunks.last().is_some_and(|c| !c.old_lines.is_empty() || !c.new_lines.is_empty()) {
            return Err(expected_at_at(no, raw));
        }
        Err(unexpected_update_line(no, raw))
    }
}

fn check_path(p: &str, no: usize) -> Result<PathBuf, String> {
    if p.is_empty() {
        return Err(format!("第 {no} 行：文件路径不能为空"));
    }
    Ok(PathBuf::from(p))
}

/// 取当前变更块；块尚未开始时先开新块（Update 块首个 @@ 之前的行属同一块）。
fn current_chunk(chunks: &mut Vec<UpdateChunk>) -> &mut UpdateChunk {
    if chunks.is_empty() {
        chunks.push(UpdateChunk::default());
    }
    chunks.last_mut().expect("刚推入块，必非空")
}

fn unexpected_update_line(no: usize, line: &str) -> String {
    format!(
        "第 {no} 行：更新块中出现意外行：'{line}'，\
         每行须以 ' '（上下文）、'+'（新增）或 '-'（删除）开头"
    )
}

fn expected_at_at(no: usize, line: &str) -> String {
    format!("第 {no} 行：更新块须以 @@ 上下文行开始，实际为：'{line}'")
}

/// 完整解析补丁文本；格式错误返回带行号的说明，不触碰任何文件。
fn parse_patch(text: &str) -> Result<Vec<Hunk>, String> {
    let lines: Vec<&str> = text.trim().lines().collect();
    if lines.is_empty() {
        return Err("补丁为空（或全是空白）：第一行必须是「*** Begin Patch」".into());
    }
    let mut parser = Parser { hunks: Vec::new(), mode: Mode::NotStarted };
    for (i, raw) in lines.into_iter().enumerate() {
        parser.process_line(raw, i + 1)?;
    }
    if parser.mode != Mode::Ended {
        return Err(format!("补丁最后一行必须是「{END_MARKER}」"));
    }
    Ok(parser.hunks)
}

// ---- 行级匹配与替换（Codex file_update + seek_sequence）----

/// 一次替换：(起始行下标, 被替换行数, 新行)。
type Replacement = (usize, usize, Vec<String>);

/// 在 lines 中自 start 起查找 pattern 的首个匹配下标；按完整搜索范围依次尝试
/// 四级容差（坑002：上下文应足够定位；029：追加第四级）：
/// 1. 精确 → 2. 忽略行尾空白 → 3. 忽略两端空白 → 4. 忽略两端空白＋Unicode
/// 标点/空格映射（[`map_unicode_punct`]，仅比较用，不改写任何缓冲）。
/// 每级先扫描完整允许范围再进入下一级——前面的宽松匹配不得抢走后面的较严格
/// 匹配。eof 为真时先尝试把 pattern 锚定在文件末尾。
fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let last_start = lines.len() - pattern.len();
    let search_start = if eof { last_start.max(start) } else { start };
    if search_start > last_start {
        return None;
    }
    for i in search_start..=last_start {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    for i in search_start..=last_start {
        if pattern
            .iter()
            .enumerate()
            .all(|(k, p)| lines[i + k].trim_end() == p.trim_end())
        {
            return Some(i);
        }
    }
    for i in search_start..=last_start {
        if pattern.iter().enumerate().all(|(k, p)| lines[i + k].trim() == p.trim()) {
            return Some(i);
        }
    }
    for i in search_start..=last_start {
        if pattern
            .iter()
            .enumerate()
            .all(|(k, p)| level4_line_eq(&lines[i + k], p))
        {
            return Some(i);
        }
    }
    None
}

/// 第四级行比较：两侧先按既有 `trim` 规则、再把 28 个 Unicode 标点/空格码点
/// 映射为 ASCII 对应物后逐字符比较（两侧都映射——Unicode↔ASCII 及同组内部
/// 均可匹配；中文、大小写与未列出的行内字符保持区分，029 规格 §1.3）。
fn level4_line_eq(a: &str, b: &str) -> bool {
    a.trim().chars().map(map_unicode_punct).eq(b.trim().chars().map(map_unicode_punct))
}

/// 029 固定映射表（上游 seek_sequence 的 Unicode 归一化，28 个码点）：
/// 各组 Unicode 破折号→`-`、弯单引号→`'`、弯双引号→`"`、特殊空格→` `。
/// 仅用于第四级**定位比较**，不改写源文件缓冲、路径或补丁新增文本。
fn map_unicode_punct(c: char) -> char {
    match c {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}'..='\u{201B}' => '\'',
        '\u{201C}'..='\u{201F}' => '"',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

/// 计算把 original_lines 变换为新内容所需的替换序列（按下标升序、互不重叠）。
fn compute_replacements(
    original_lines: &[String],
    path_label: &str,
    chunks: &[UpdateChunk],
) -> Result<Vec<Replacement>, String> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // @@ 定位行：把后续匹配起点推进到它之后
        if let Some(ctx) = &chunk.change_context {
            match seek_sequence(original_lines, std::slice::from_ref(ctx), line_index, false) {
                Some(idx) => line_index = idx + 1,
                None => {
                    return Err(format!("在 {path_label} 中找不到定位上下文「{ctx}」"))
                }
            }
        }

        if chunk.old_lines.is_empty() {
            // 纯插入块（只有 + 行）：追加到文件末尾
            replacements.push((original_lines.len(), 0, chunk.new_lines.clone()));
            continue;
        }

        // 末尾空行常代表“最终换行”哨兵：直接找不到时去掉后重试
        let mut pattern: &[String] = &chunk.old_lines;
        let mut new_slice: &[String] = &chunk.new_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        let Some(start_idx) = found else {
            return Err(format!(
                "在 {path_label} 中找不到要修改的行（上下文不匹配）：\n{}",
                chunk.old_lines.join("\n")
            ));
        };

        // 上下文行保留原始行（含原行尾），替换只作用于之间的差量
        let mut old_start = 0;
        let mut new_start = 0;
        for &(old_ctx, new_ctx) in &chunk.context_line_indices {
            // 哨兵重试可能已裁掉末尾上下文
            if old_ctx >= pattern.len() || new_ctx >= new_slice.len() {
                break;
            }
            if old_start != old_ctx || new_start != new_ctx {
                replacements.push((
                    start_idx + old_start,
                    old_ctx - old_start,
                    new_slice[new_start..old_ctx].to_vec(),
                ));
            }
            old_start = old_ctx + 1;
            new_start = new_ctx + 1;
        }
        if old_start != pattern.len() || new_start != new_slice.len() {
            replacements.push((
                start_idx + old_start,
                pattern.len() - old_start,
                new_slice[new_start..].to_vec(),
            ));
        }
        line_index = start_idx + pattern.len();
    }

    replacements.sort_by_key(|(idx, _, _)| *idx);
    Ok(replacements)
}

// ---- 行尾保持的源文件重写（Codex text_file）----

#[derive(Clone, Copy, PartialEq, Debug)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

struct SourceLine {
    text: String,
    ending: Option<LineEnding>,
}

struct SourceFile {
    lines: Vec<SourceLine>,
    /// 插入/替换行的约定风格：原文件首个换行；无换行时 LF。
    preferred_ending: LineEnding,
}

impl SourceFile {
    /// 按换行拆分为逻辑行并保留各自行尾（经 [`line_ranges`] 原语——与行数门
    /// 同一实现）。
    fn parse(contents: &str) -> Self {
        let mut lines = Vec::new();
        let mut preferred: Option<LineEnding> = None;
        for (text, ending) in line_ranges(contents) {
            if let Some(e) = ending {
                preferred.get_or_insert(e);
            }
            lines.push(SourceLine { text: text.to_string(), ending });
        }
        Self { lines, preferred_ending: preferred.unwrap_or(LineEnding::Lf) }
    }

    fn line_texts(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.text.clone()).collect()
    }

    /// 应用按下标升序、互不重叠的替换：未涉及行保留原行尾，插入行用约定风格，
    /// 最后为所有行补齐行尾（更新后补末尾换行）。
    fn apply_replacements(&mut self, replacements: &[Replacement]) {
        let mut source_lines = std::mem::take(&mut self.lines).into_iter();
        let mut new_lines = Vec::new();
        let mut source_index = 0;
        for (start_idx, old_len, new_segment) in replacements {
            debug_assert!(*start_idx >= source_index, "替换序列必须升序且不重叠");
            for line in source_lines.by_ref().take(start_idx - source_index) {
                new_lines.push(line);
            }
            for _ in source_lines.by_ref().take(*old_len) {}
            new_lines.extend(new_segment.iter().map(|text| SourceLine {
                text: text.clone(),
                ending: Some(self.preferred_ending),
            }));
            source_index = start_idx + old_len;
        }
        new_lines.extend(source_lines);
        self.lines = new_lines;
        for line in &mut self.lines {
            line.ending.get_or_insert(self.preferred_ending);
        }
    }

    fn into_contents(self) -> String {
        let mut contents = String::new();
        for line in self.lines {
            contents.push_str(&line.text);
            if let Some(ending) = line.ending {
                contents.push_str(ending.as_str());
            }
        }
        contents
    }
}

/// 计算更新后的完整新内容（匹配失败即报错，不触碰当前文件）。
fn compute_new_content(
    original: &str,
    path_label: &str,
    chunks: &[UpdateChunk],
) -> Result<String, String> {
    let mut source = SourceFile::parse(original);
    let original_lines = source.line_texts();
    let replacements = compute_replacements(&original_lines, path_label, chunks)?;
    source.apply_replacements(&replacements);
    Ok(source.into_contents())
}

// ---- 顺序执行器 ----

/// 单块失败：区分“未发生写入”（清单仍可信）与“变更操作失败”（须重读）。
struct Failure {
    msg: String,
    state_known: bool,
}

impl Failure {
    /// 校验/解析/读取/匹配失败：未发生写入，清单完整可信。
    fn validation(msg: String) -> Self {
        Self { msg, state_known: true }
    }
    /// 写入/删除/建目录失败：目标可能被部分修改（坑003），清单不可信。
    fn mutating(msg: String) -> Self {
        Self { msg, state_known: false }
    }
}

/// 相对路径基于会话当前目录解析；绝对路径原样使用。
fn resolve_target(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 统一打开样板（审查 #3 收敛）：`O_RDONLY|O_NONBLOCK|O_CLOEXEC|extra_flags`。
/// FIFO/设备打开本身不阻塞；打开成功即持有实际读写的 inode，随后对该 fd
/// fstat 判型，类型验证与 I/O 绑定同一对象（安全报告 F2 收口）。
fn open_path(target: &Path, extra_flags: libc::c_int) -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | extra_flags,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}

/// Update 读入（跟随符号链接的既有语义）。
fn open_validated_regular(target: &Path) -> std::io::Result<std::fs::File> {
    open_path(target, 0)
}

/// 有界源读入核心（Update/Move 共用，审查 #7 收敛）：fd 级 fstat 预检＋take
/// 有限读取＋字节门（N5）；并核对**读取字节数与 fstat 尺寸一致**——常规文件
/// 的读取结果不可能偏离其元数据尺寸：超出＝procfs/sysfs 类伪文件（S_IFREG、
/// st_size=0 而读取有内容，安全修复 F-2），不足＝fstat 与读取之间被并发截断
/// （审查 #8，data-loss：短读残片曾被当完整内容移动并删源）。两向都保守拒绝。

enum BoundedReadError {
    /// fstat 尺寸超过上限
    OversizeSize(u64),
    /// 读取字节数超过上限
    TooManyBytes,
    /// 读取字节数与 fstat 尺寸不符（伪文件或读入窗口内被并发截断/追加）
    Inconsistent { got: usize, want: u64 },
    Io(std::io::Error),
}

fn read_bounded(file: &std::fs::File) -> Result<(Vec<u8>, u64), BoundedReadError> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstat(file.as_raw_fd(), &mut st) };
    if st.st_size as u64 > MAX_FILE_BYTES as u64 {
        return Err(BoundedReadError::OversizeSize(st.st_size as u64));
    }
    // 审查 #8 截断回归的确定性屏障位：fstat 之后、读取之前（未启用时零开销）
    #[cfg(test)]
    move_seam::barrier("before_read");
    let mut bytes = Vec::new();
    let mut limited = (file).take(MAX_FILE_BYTES as u64 + 1);
    std::io::Read::read_to_end(&mut limited, &mut bytes).map_err(BoundedReadError::Io)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(BoundedReadError::TooManyBytes);
    }
    if bytes.len() as u64 != st.st_size as u64 {
        return Err(BoundedReadError::Inconsistent {
            got: bytes.len(),
            want: st.st_size as u64,
        });
    }
    Ok((bytes, st.st_size as u64))
}

/// UTF-8 解码＋行数门（Update/Move 共用）。`label` 已含完整对象描述与路径。
fn decode_bounded(bytes: Vec<u8>, label: &str) -> Result<String, String> {
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return Err(format!("{label}不是有效的 UTF-8 文本（不支持二进制）")),
    };
    let n = logical_lines(&s);
    if n > MAX_FILE_LINES {
        return Err(format!("{label} 为 {n} 行，超过 {MAX_FILE_LINES} 行读入上限"));
    }
    Ok(s)
}

/// 生成内容写前上限（Update/Move 共用，N5）：字节数＋行数门；超限不得写入
/// 或删除源。`label` 为对象描述（如「目标 X（源 Y）」）。
fn check_generated_limits(new_content: &str, label: &str) -> Result<(), String> {
    if new_content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "生成内容 {} 字节超过 {MAX_FILE_BYTES} 字节（4 MiB）上限（{label}）",
            new_content.len()
        ));
    }
    let n = logical_lines(new_content);
    if n > MAX_FILE_LINES {
        return Err(format!("生成内容 {n} 行超过 {MAX_FILE_LINES} 行上限（{label}）"));
    }
    Ok(())
}

/// fd 指向的对象是否常规文件（fstat 绑定打开的 inode，不受路径替换影响）。
fn fd_is_regular(f: &std::fs::File) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstat(f.as_raw_fd(), &mut st) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG }
}

/// fd 指向对象的 (st_dev, st_ino)——用于确认两个句柄确为同一 inode。
fn fd_inode(f: &std::fs::File) -> Option<(u64, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(f.as_raw_fd(), &mut st) } != 0 {
        return None;
    }
    Some((st.st_dev as u64, st.st_ino as u64))
}

/// 写入绑定读入时已判型的 inode：经 `/proc/self/fd/<fd>` 重开同一文件
/// （O_TRUNC 只作用于该 inode），重开后二次 fstat 确认仍为常规文件。
/// 路径被替换为 FIFO 时本写入不打开路径，既不阻塞也不会截断占位对象。
fn write_via_validated_fd(read_fd: &std::fs::File, data: &[u8]) -> std::io::Result<()> {
    let path = std::ffi::CString::new(format!("/proc/self/fd/{}", read_fd.as_raw_fd()))?;
    let wfd =
        unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_TRUNC | libc::O_CLOEXEC) };
    if wfd < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(std::io::Error::new(
                e.kind(),
                format!("{e}（写入经 /proc/self/fd 绑定原对象，需要 /proc 已挂载）"),
            ));
        }
        return Err(e);
    }
    let mut w = unsafe { std::fs::File::from_raw_fd(wfd) };
    // 纵深防御：写句柄须与读入句柄为同一 inode 且仍为常规文件
    if fd_inode(&w) != fd_inode(read_fd) || !fd_is_regular(&w) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "写入句柄与已判型对象不一致",
        ));
    }
    w.write_all(data)
}

/// F2 窗口测试同步接缝（**仅测试构建**，不构成公开接口/参数）：
/// Update 完成打开＋判型＋读入后、写入前，`before_write` 写出 reached 标记并等待
/// release 文件出现——让控制端在确定性的"写前"时点完成目标替换。
/// 由 `apply_patch::tests::f2_swap_barrier_*` 的子进程模式启用。
#[cfg(test)]
mod f2_seam {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ARMED: AtomicBool = AtomicBool::new(false);

    /// 启用接缝：仅测试子进程在调用 apply_patch 前设置。
    pub fn arm() {
        ARMED.store(true, Ordering::SeqCst);
    }

    /// 写入前屏障：启用时通知控制端已到达"判型＋读入完成、写入未开始"，
    /// 然后阻塞等待 release。协调目录经 `TCHAT_F2_SEAM_DIR` 传递（仅测试构建内读取）。
    pub fn before_write() {
        if !ARMED.load(Ordering::SeqCst) {
            return;
        }
        let dir = std::env::var("TCHAT_F2_SEAM_DIR").unwrap_or_default();
        assert!(!dir.is_empty(), "接缝已启用但缺少协调目录");
        let d = std::path::PathBuf::from(dir);
        std::fs::write(d.join("reached"), b"").expect("写出 reached 标记");
        let release = d.join("release");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !release.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "接缝等待 release 超时（控制端未放行）"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// 012 展示用旧文读入（Delete 目标）：有界（MAX_FILE_BYTES）且须为 UTF-8；
/// 超限/不可读/非文本返回 None——展示层明确降级，不影响删除本身。
fn read_display_content(target: &Path) -> Option<String> {
    let file = std::fs::File::open(target).ok()?;
    let mut bytes = Vec::new();
    let mut limited = (&file).take(MAX_FILE_BYTES as u64 + 1);
    std::io::Read::read_to_end(&mut limited, &mut bytes).ok()?;
    if bytes.len() > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn apply_hunk(
    hunk: &Hunk,
    base: &Path,
    changed: &mut Vec<ChangedFile>,
    display: &mut Vec<crate::prompt::PatchChange>,
) -> Result<(), Failure> {
    match hunk {
        Hunk::Add { path, content } => {
            let target = resolve_target(base, path);
            // N5：生成内容行数上限（字节上限由 patch ≤1 MiB 天然保证）
            if logical_lines(content) > MAX_FILE_LINES {
                return Err(Failure::validation(format!(
                    "新增内容 {} 行超过 {MAX_FILE_LINES} 行上限（目标 {}）",
                    logical_lines(content),
                    target.display()
                )));
            }
            // 新增不得覆盖已有目标（含悬空符号链接；create_new 再兜底竞态）
            if std::fs::symlink_metadata(&target).is_ok() {
                return Err(Failure::validation(format!(
                    "新增目标已存在：{}",
                    target.display()
                )));
            }
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Err(Failure::mutating(format!(
                            "创建父目录失败（目标 {}）：{e}",
                            target.display()
                        )));
                    }
                }
            }
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(Failure::validation(format!(
                        "新增目标已存在：{}",
                        target.display()
                    )));
                }
                Err(e) => {
                    return Err(Failure::mutating(format!(
                        "写入新增文件失败 {}：{e}",
                        target.display()
                    )));
                }
            };
            if let Err(e) = file.write_all(content.as_bytes()) {
                return Err(Failure::mutating(format!(
                    "写入新增文件失败 {}：{e}",
                    target.display()
                )));
            }
            let label = path_label(&target);
            changed.push(ChangedFile { path: label.clone(), action: Action::Add });
            display.push(crate::prompt::PatchChange {
                path: label,
                kind: crate::prompt::PatchChangeKind::Add { content: content.clone() },
            });
            Ok(())
        }
        Hunk::Delete { path } => {
            let target = resolve_target(base, path);
            let meta = match std::fs::symlink_metadata(&target) {
                Ok(m) => m,
                Err(_) => {
                    return Err(Failure::validation(format!(
                        "删除目标不存在：{}",
                        target.display()
                    )));
                }
            };
            if meta.is_dir() {
                return Err(Failure::validation(format!(
                    "删除目标是目录，不是文件：{}",
                    target.display()
                )));
            }
            // 012：先读旧全文（有界展示读入），再删除——展示差异来自真实旧内容
            let old_content = read_display_content(&target);
            match std::fs::remove_file(&target) {
                Ok(()) => {
                    let label = path_label(&target);
                    changed.push(ChangedFile { path: label.clone(), action: Action::Delete });
                    display.push(crate::prompt::PatchChange {
                        path: label,
                        kind: crate::prompt::PatchChangeKind::Delete { content: old_content },
                    });
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Err(Failure::validation(format!("删除目标不存在：{}", target.display())))
                }
                Err(e) => Err(Failure::mutating(format!(
                    "删除失败 {}：{e}",
                    target.display()
                ))),
            }
        }
        Hunk::Update { path, move_to, chunks } => {
            // 029：带 Move to 的块走移动执行（源读取、目标创建/写入、源删除）；
            // 无 Move 的普通 Update 保持既有语义。
            if let Some(dest) = move_to {
                return apply_move(path, dest, chunks, base, changed, display);
            }
            let target = resolve_target(base, path);
            // F2：Update 只读写常规文件。先以非阻塞方式打开（FIFO/设备打开不阻塞），
            // 再对打开的 fd fstat 判型——判型绑定实际读写的 inode；悬空链接仍报
            // "目标不存在"（open 返回 ENOENT），非常规目标在任何 I/O 前拒绝。
            let file = match open_validated_regular(&target) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(Failure::validation(format!(
                        "修改目标不存在：{}",
                        target.display()
                    )));
                }
                Err(e) => {
                    return Err(Failure::validation(format!(
                        "读取修改目标失败 {}：{e}",
                        target.display()
                    )));
                }
            };
            if !fd_is_regular(&file) {
                return Err(Failure::validation(format!(
                    "修改目标不是常规文件（拒绝 FIFO/设备/目录等非常规目标）：{}",
                    target.display()
                )));
            }
            // N5：fd 级大小预检＋有限读取＋读入一致性（审查 #7/#8：与 Move 共用
            // read_bounded——伪文件/读入窗口内并发截断两向都保守拒绝）
            let bytes = match read_bounded(&file) {
                Ok((bytes, _)) => bytes,
                Err(BoundedReadError::OversizeSize(size)) => {
                    return Err(Failure::validation(format!(
                        "修改目标 {} 为 {size} 字节，超过 {MAX_FILE_BYTES} 字节（4 MiB）读入上限",
                        target.display()
                    )));
                }
                Err(BoundedReadError::TooManyBytes) => {
                    return Err(Failure::validation(format!(
                        "修改目标 {} 超过 {MAX_FILE_BYTES} 字节（4 MiB）读入上限",
                        target.display()
                    )));
                }
                Err(BoundedReadError::Inconsistent { got, want }) => {
                    return Err(Failure::validation(format!(
                        "修改目标 {} 读入不一致（读取 {got} 字节与元数据尺寸 {want} 不符，\
                         可能在读入窗口内被并发修改或为伪文件）：拒绝处理",
                        target.display()
                    )));
                }
                Err(BoundedReadError::Io(e)) => {
                    return Err(Failure::validation(format!(
                        "读取修改目标失败 {}：{e}",
                        target.display()
                    )));
                }
            };
            let label = format!("修改目标 {}", target.display());
            let original = match decode_bounded(bytes, &label) {
                Ok(s) => s,
                Err(msg) => return Err(Failure::validation(msg)),
            };
            let new_content = match compute_new_content(&original, &target.display().to_string(), chunks)
            {
                Ok(c) => c,
                Err(msg) => return Err(Failure::validation(msg)),
            };
            // N5：生成内容在写入/截断目标之前检查字节数与行数（超限不改变目标字节）
            check_generated_limits(&new_content, &format!("目标 {}", target.display()))
                .map_err(Failure::validation)?;
            #[cfg(test)]
            f2_seam::before_write();
            match write_via_validated_fd(&file, new_content.as_bytes()) {
                Ok(()) => {
                    // 012：展示差异＝对真实前后缓冲生成的统一 diff（不从补丁文本反推）
                    let diff = diffy::create_patch(&original, &new_content).to_string();
                    let label = path_label(&target);
                    changed.push(ChangedFile { path: label.clone(), action: Action::Update });
                    display.push(crate::prompt::PatchChange {
                        path: label,
                        kind: crate::prompt::PatchChangeKind::Update { diff },
                    });
                    Ok(())
                }
                Err(e) => Err(Failure::mutating(format!(
                    "写入修改目标失败 {}：{e}",
                    target.display()
                ))),
            }
        }
    }
}

/// 029 Move 执行（规格 §1.4 顺序）：
/// 1. 同路径与目标存在预检（词法 `.`/`..` 归一；其余别名由排他创建兜底）；
/// 2. 源经 `O_NOFOLLOW` 打开并 fd 判型（拒绝末端符号链接/非常规文件），
///    有界读入＋UTF-8＋行数校验（与普通 Update 同界）；
/// 3. 内容计算：纯移动逐字节保留（含混合行尾与缺失末尾换行）；带修改块时按
///    匹配规则生成新内容并做写前上限检查；
/// 4. 目标父目录沿用 Add 创建策略；`create_new` 排他创建＋完整写入——完整写入
///    成功**立即**记目标 `add`（012 展示＝目标真实新内容）；半写不记 add；
/// 5. 删除源前复核源路径仍对应读入对象（更换则保留目标 add、不删替换对象）；
/// 6. 删除源成功记源 `delete`（展示＝源真实旧内容）；失败保留目标 add、不清理。
/// 不承诺 inode/权限位/时间戳随源保留（文本内容的新建＋删除）。
fn apply_move(
    src_path: &Path,
    dest_path: &Path,
    chunks: &[UpdateChunk],
    base: &Path,
    changed: &mut Vec<ChangedFile>,
    display: &mut Vec<crate::prompt::PatchChange>,
) -> Result<(), Failure> {
    let src = resolve_target(base, src_path);
    let dst = resolve_target(base, dest_path);
    // 阶段化错误（规格 §1.4.5）：单一实现点，全部出错点经 ctx/both 构造，
    // 均含源、目标两个可确定绝对路径（审查 #12 收敛）
    let ctx = |stage: &str| {
        format!("{stage}（源 {}，目标 {}）", src.display(), dst.display())
    };
    let both = |stage: &str, detail: &dyn std::fmt::Display| {
        format!("{}：{detail}", ctx(stage))
    };

    // 1. 同路径拒绝：词法归一后比较（`./`、`..`）；其余别名等价由排他创建兜底
    if lexical_normalize(&src) == lexical_normalize(&dst) {
        return Err(Failure::validation(ctx("移动源与目标为同一路径，按目标冲突拒绝")));
    }
    // 目标存在预检：任何目录项（常规/目录/符号链接/悬空/硬链接）均拒绝
    if std::fs::symlink_metadata(&dst).is_ok() {
        return Err(Failure::validation(ctx("移动目标已存在，禁止覆盖")));
    }

    // 2. 源打开＋判型：O_NOFOLLOW——末端符号链接（含悬空，ELOOP）不跟随，
    //    避免"复制链接所指内容后删除链接本体"（普通 Update 跟随语义不变）
    let file = match open_source_nofollow(&src) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Failure::validation(format!(
                "移动源不存在（目标 {}）：{}",
                dst.display(),
                src.display()
            )));
        }
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(Failure::validation(ctx("移动源是符号链接（含悬空），拒绝移动")));
        }
        Err(e) => return Err(Failure::validation(both("打开移动源失败", &e))),
    };
    if !fd_is_regular(&file) {
        return Err(Failure::validation(ctx("移动源不是常规文件（拒绝 FIFO/设备/目录等）")));
    }
    // N5＋F-2＋审查 #8：fd 级大小预检＋有限读取＋读入一致性（与 Update 共用
    // read_bounded——超出尺寸＝procfs/sysfs 伪文件，不足＝读入窗口内被并发
    // 截断；两向都保守拒绝，防止把环境变量物化或把截断残片当完整内容移动）
    let bytes = match read_bounded(&file) {
        Ok((bytes, _)) => bytes,
        Err(BoundedReadError::OversizeSize(size)) => {
            return Err(Failure::validation(format!(
                "移动源 {} 为 {size} 字节，超过 {MAX_FILE_BYTES} 字节（4 MiB）读入上限",
                src.display()
            )));
        }
        Err(BoundedReadError::TooManyBytes) => {
            return Err(Failure::validation(format!(
                "移动源 {} 超过 {MAX_FILE_BYTES} 字节（4 MiB）读入上限",
                src.display()
            )));
        }
        Err(BoundedReadError::Inconsistent { got, want }) => {
            return Err(Failure::validation(format!(
                "移动源 {} 是伪文件或读入窗口内被并发截断（读取 {got} 字节与元数据尺寸 \
                 {want} 不符，拒绝移动 procfs/sysfs 类对象或并发变化的源；目标 {}）",
                src.display(),
                dst.display()
            )));
        }
        Err(BoundedReadError::Io(e)) => {
            return Err(Failure::validation(both("读取移动源失败", &e)));
        }
    };
    let original = match decode_bounded(bytes, &format!("移动源 {}（目标 {}）", src.display(), dst.display())) {
        Ok(s) => s,
        Err(msg) => return Err(Failure::validation(msg)),
    };

    // 3. 内容计算：纯移动逐字节保留；带修改块按匹配规则生成（失败不触任何文件）
    let new_content = if chunks.is_empty() {
        original.clone()
    } else {
        match compute_new_content(&original, &src.display().to_string(), chunks) {
            Ok(c) => c,
            Err(msg) => return Err(Failure::validation(msg)),
        }
    };
    // 写前上限检查：超限不得创建目标或删除源（N5；纯移动内容＝已过读入门的
    // 原文，跳过重复检查——审查 #10）
    if !chunks.is_empty() {
        check_generated_limits(&new_content, &format!("目标 {}（源 {}）", dst.display(), src.display()))
            .map_err(Failure::validation)?;
    }

    // 4. 目标父目录沿用 Add 创建策略（失败＝已尝试变更目录，状态未知）
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(Failure::mutating(both("创建移动目标父目录失败", &e)));
            }
        }
    }
    // 排他创建＋完整写入：create_new 原子拒绝既有目标（含预检后的抢占竞态）
    #[cfg(test)]
    move_seam::barrier("before_create");
    let mut out = match std::fs::OpenOptions::new().write(true).create_new(true).open(&dst) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Failure::validation(ctx("移动目标已存在，禁止覆盖")));
        }
        Err(e) => return Err(Failure::mutating(both("创建移动目标失败", &e))),
    };
    if let Err(e) = out.write_all(new_content.as_bytes()) {
        // 半写：目标残片保留供观察，不记 add；源内容完整（尚未动源）
        return Err(Failure::mutating(both("写入移动目标失败", &e)));
    }
    drop(out);
    // 完整写入成功：立即记目标 add；展示＝目标真实新内容
    let dst_label = path_label(&dst);
    changed.push(ChangedFile { path: dst_label.clone(), action: Action::Add });
    display.push(crate::prompt::PatchChange {
        path: dst_label,
        kind: crate::prompt::PatchChangeKind::Add { content: new_content.clone() },
    });

    // 5. 删源前复核：源路径仍对应读入时的对象（替换窗口测试经 move_seam 屏障）
    #[cfg(test)]
    move_seam::barrier("before_unlink");
    let verify = match open_source_nofollow(&src) {
        Ok(f) => f,
        Err(e) => return Err(Failure::mutating(both("删除前复核移动源失败", &e))),
    };
    if fd_inode(&verify) != fd_inode(&file) {
        return Err(Failure::mutating(format!(
            "移动源对象在删除前被更换，已保留目标 add、不删除替换对象（源 {}，目标 {}）",
            src.display(),
            dst.display()
        )));
    }
    drop(verify);
    drop(file);

    // 6. 删除源：成功记 delete（展示＝源真实旧内容）；失败保留目标 add、不清理
    match std::fs::remove_file(&src) {
        Ok(()) => {
            let src_label = path_label(&src);
            changed.push(ChangedFile { path: src_label.clone(), action: Action::Delete });
            display.push(crate::prompt::PatchChange {
                path: src_label,
                kind: crate::prompt::PatchChangeKind::Delete { content: Some(original) },
            });
            Ok(())
        }
        Err(e) => Err(Failure::mutating(both("删除移动源失败", &e))),
    }
}

/// Move 源打开：加 `O_NOFOLLOW`——末端符号链接（含悬空）一律 ELOOP 拒绝、
/// 不跟随（普通 Update 的 [`open_validated_regular`] 保持跟随语义不变）。
fn open_source_nofollow(target: &Path) -> std::io::Result<std::fs::File> {
    open_path(target, libc::O_NOFOLLOW)
}

/// 词法归一（消 `.`/`..`）比较两路径是否同一目标——用于移动的自路径拒绝；
/// 不触文件系统（符号链接别名等价由 `create_new` 排他创建兜底）。
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::Prefix(_) => {}
        }
    }
    out
}

/// 029 Move 窗口测试同步接缝（**仅测试构建**，与 f2_seam 同构）：具名屏障，
/// 控制端在确定性的时点完成抢占/替换——
/// - `before_create`：目标预检后、排他创建前（目标抢占竞态，A5）；
/// - `before_unlink`：目标完整写入并记录 add 后、删源复核前（源替换，A7b）。
/// 由 `apply_patch::tests::move_*` 的子进程模式启用。
#[cfg(test)]
mod move_seam {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ARMED: AtomicBool = AtomicBool::new(false);

    /// 启用接缝：仅测试子进程在调用 apply_patch 前设置。
    pub fn arm() {
        ARMED.store(true, Ordering::SeqCst);
    }

    /// 具名屏障：启用时写出 `reached-<name>` 并阻塞等待 `release-<name>`。
    /// 协调目录经 `TCHAT_MOVE_SEAM_DIR` 传递。
    pub fn barrier(name: &str) {
        if !ARMED.load(Ordering::SeqCst) {
            return;
        }
        let dir = std::env::var("TCHAT_MOVE_SEAM_DIR").unwrap_or_default();
        assert!(!dir.is_empty(), "接缝已启用但缺少协调目录");
        let d = std::path::PathBuf::from(dir);
        std::fs::write(d.join(format!("reached-{name}")), b"").expect("写出 reached 标记");
        let release = d.join(format!("release-{name}"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !release.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "接缝等待 release-{name} 超时（控制端未放行）"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// 展示路径标签：与结果 changed_files 的 `path.display().to_string()` 同源。
fn path_label(target: &Path) -> String {
    target.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::FileTypeExt;

    // ---- ApplyPatchArgs::from_value：参数校验 ----

    #[test]
    fn args_accept_patch_field() {
        let a = ApplyPatchArgs::from_value(&json!({ "patch": "x" })).unwrap();
        assert_eq!(a, ApplyPatchArgs { patch: "x".into() });
    }

    #[test]
    fn args_reject_bad_values() {
        assert!(ApplyPatchArgs::from_value(&json!(5)).unwrap_err().contains("对象"));
        assert!(ApplyPatchArgs::from_value(&json!("p")).unwrap_err().contains("对象"));
        assert!(ApplyPatchArgs::from_value(&json!({})).unwrap_err().contains("缺少必填参数"));
        assert!(
            ApplyPatchArgs::from_value(&json!({ "patch": null }))
                .unwrap_err()
                .contains("缺少必填参数")
        );
        assert!(ApplyPatchArgs::from_value(&json!({ "patch": 7 })).unwrap_err().contains("字符串"));
        assert!(ApplyPatchArgs::from_value(&json!({ "patch": "x", "evil": 1 }))
            .unwrap_err()
            .contains("未知参数"));
    }

    // ---- 输出契约：四字段始终存在、动作小写 ----

    #[test]
    fn result_serializes_spec_shape() {
        let r = ApplyPatchResult {
            ok: false,
            changed_files: vec![ChangedFile { path: "/tmp/x".into(), action: Action::Add }],
            state_known: false,
            error: Some("e".into()),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "ok": false,
                "changed_files": [{ "path": "/tmp/x", "action": "add" }],
                "state_known": false,
                "error": "e"
            })
        );
        let ok = ApplyPatchResult {
            ok: true,
            changed_files: vec![],
            state_known: true,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({ "ok": true, "changed_files": [], "state_known": true, "error": null })
        );
    }

    // ---- 解析器：结构与宽容性 ----

    fn parse_ok(text: &str) -> Vec<Hunk> {
        parse_patch(text).expect("应解析成功")
    }

    #[test]
    fn parse_full_patch_structure() {
        let hunks = parse_ok(
            "*** Begin Patch\n\
             *** Add File: path/add.py\n+abc\n+def\n\
             *** Delete File: path/delete.py\n\
             *** Update File: path/update.py\n\
             @@ def f():\n-    pass\n+    return 123\n\
             *** End Patch",
        );
        assert_eq!(hunks.len(), 3);
        assert_eq!(
            hunks[0],
            Hunk::Add { path: PathBuf::from("path/add.py"), content: "abc\ndef\n".into() }
        );
        assert_eq!(hunks[1], Hunk::Delete { path: PathBuf::from("path/delete.py") });
        match &hunks[2] {
            Hunk::Update { path, chunks, .. } => {
                assert_eq!(path, &PathBuf::from("path/update.py"));
                assert_eq!(chunks.len(), 1);
                let c = &chunks[0];
                assert_eq!(c.change_context.as_deref(), Some("def f():"));
                assert_eq!(c.old_lines, vec!["    pass".to_string()]);
                assert_eq!(c.new_lines, vec!["    return 123".to_string()]);
                assert!(!c.is_end_of_file);
            }
            other => panic!("应为 Update 块：{other:?}"),
        }
    }

    #[test]
    fn parse_update_without_at_at_starts_first_chunk() {
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: file2.py\n import foo\n+bar\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { chunks, .. } => {
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].change_context, None);
                assert_eq!(chunks[0].old_lines, vec!["import foo".to_string()]);
                assert_eq!(chunks[0].new_lines, vec!["import foo".to_string(), "bar".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_context_and_empty_lines() {
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: f\n@@\n a\n\n b\n-x\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { chunks, .. } => {
                let c = &chunks[0];
                assert_eq!(c.old_lines, vec!["a".to_string(), String::new(), "b".to_string(), "x".to_string()]);
                assert_eq!(c.new_lines, vec!["a".to_string(), String::new(), "b".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_end_of_file_marker_sets_flag() {
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: f.txt\n@@\n+quux\n*** End of File\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { chunks, .. } => {
                assert!(chunks[0].is_end_of_file);
                assert!(chunks[0].old_lines.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_crlf_patch_text_matches_lf() {
        let crlf = "*** Begin Patch\r\n*** Add File: a.txt\r\n+hi\r\n*** End Patch\r\n";
        assert_eq!(
            parse_ok(crlf),
            vec![Hunk::Add { path: PathBuf::from("a.txt"), content: "hi\n".into() }]
        );
    }

    #[test]
    fn parse_marker_whitespace_is_lenient() {
        // 顶层块头按整行 trim 匹配；Add 路径后的尾随空白被裁剪
        let hunks = parse_ok(
            " *** Begin Patch \n*** Add File: a.txt  \n+hi\n *** End Patch ",
        );
        assert_eq!(
            hunks,
            vec![Hunk::Add { path: PathBuf::from("a.txt"), content: "hi\n".into() }]
        );
    }

    #[test]
    fn parse_indented_update_marker_is_context() {
        // Update 模式下带前导空格的块头是上下文行，不会开启新块
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: a.txt\n@@\n-old a\n+new a\n *** Update File: b.txt\n*** End Patch",
        );
        assert_eq!(hunks.len(), 1, "带前导空格的块头不应开启新块");
    }

    #[test]
    fn parse_empty_patch_yields_no_hunks() {
        assert_eq!(parse_ok("*** Begin Patch\n*** End Patch"), Vec::<Hunk>::new());
    }

    #[test]
    fn parse_rejects_invalid_patches() {
        let cases: Vec<(&str, &str)> = vec![
            ("", "为空"),
            ("bad", "第一行"),
            ("*** Begin Patch\nbad", "块头"),
            ("*** Begin Patch\n*** Add File: f\n+hi", "最后一行"),
            ("*** Begin Patch\n*** Update File: f\n*** End Patch", "为空"),
            ("*** Begin Patch\n*** Update File: f\n@@\n*** End Patch", "变更行"),
            ("*** Begin Patch\n*** Update File: f\n@@\n@@\n-x\n*** End Patch", "意外行"),
            ("*** Begin Patch\n*** Update File: f\n@@\n-old\nbad\n*** End Patch", "@@"),
            ("*** Begin Patch\n*** Add File: f\n*** End Patch", "+"),
            (
                "*** Begin Patch\n*** Add File: f\nnot-plus\n*** End Patch",
                "+",
            ),
            ("*** Begin Patch\n*** Add File: \n+x\n*** End Patch", "块头"),
            ("*** Begin Patch\n*** Add File: f\n+hi\n*** End Patch\nextra", "多余内容"),
            // ---- 029 Move 语法：非法变体（重复/错位/空目标/不属于 Update 块）----
            (
                "*** Begin Patch\n*** Update File: f\n*** Move to: g\n*** Move to: h\n@@\n-a\n+b\n*** End Patch",
                "Move",
            ),
            (
                "*** Begin Patch\n*** Update File: f\n@@\n-a\n*** Move to: g\n*** End Patch",
                "@@",
            ),
            (
                "*** Begin Patch\n*** Update File: f\n*** Move to:\n@@\n-a\n+b\n*** End Patch",
                "Move",
            ),
            (
                "*** Begin Patch\n*** Update File: f\n*** Move to:  \n*** End Patch",
                "Move",
            ),
            (
                "*** Begin Patch\n*** Add File: f\n*** Move to: g\n+x\n*** End Patch",
                "+",
            ),
            (
                "*** Begin Patch\n*** Delete File: f\n*** Move to: g\n*** End Patch",
                "块头",
            ),
        ];
        for (text, hint) in cases {
            let err = parse_patch(text).unwrap_err();
            assert!(err.contains(hint), "补丁 {text:?} 的错误应包含「{hint}」，实际：{err}");
        }
    }

    // ---- 029 M1：Move 语法正例（纯移动 / 移动并修改 / CRLF / 上下文前缀）----

    #[test]
    fn parse_move_to_accepted() {
        // 纯移动：Update＋Move to，无任何修改块
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: notes/旧名.txt\n*** Move to: archive/新名.txt\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { path, move_to, chunks } => {
                assert_eq!(path, &PathBuf::from("notes/旧名.txt"));
                assert_eq!(move_to.as_deref(), Some(Path::new("archive/新名.txt")));
                assert!(chunks.is_empty(), "纯移动不带修改块");
            }
            other => panic!("应为 Update 块：{other:?}"),
        }

        // 移动并修改：Move to 后接 @@ 修改块
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: src/old.txt\n*** Move to: src/new.txt\n@@\n-mode=dev\n+mode=test\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { path, move_to, chunks } => {
                assert_eq!(path, &PathBuf::from("src/old.txt"));
                assert_eq!(move_to.as_deref(), Some(Path::new("src/new.txt")));
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].old_lines, vec!["mode=dev".to_string()]);
                assert_eq!(chunks[0].new_lines, vec!["mode=test".to_string()]);
            }
            other => panic!("应为 Update 块：{other:?}"),
        }

        // CRLF 补丁文本中的 Move 标记
        let hunks = parse_ok(
            "*** Begin Patch\r\n*** Update File: f\r\n*** Move to: g\r\n*** End Patch\r\n",
        );
        match &hunks[0] {
            Hunk::Update { move_to, chunks, .. } => {
                assert_eq!(move_to.as_deref(), Some(Path::new("g")));
                assert!(chunks.is_empty());
            }
            other => panic!("应为 Update 块：{other:?}"),
        }
    }

    /// 带上下文前缀的 ` *** Move to: ...` 是文件内容（上下文行），不当成指令。
    #[test]
    fn parse_context_prefixed_move_marker_is_content() {
        let hunks = parse_ok(
            "*** Begin Patch\n*** Update File: f\n@@\n keep\n  *** Move to: not-a-command\n*** End Patch",
        );
        match &hunks[0] {
            Hunk::Update { chunks, .. } => {
                assert!(
                    chunks[0].old_lines.iter().any(|l| l.contains("*** Move to: not-a-command")),
                    "前缀空格的 Move 标记应作为上下文内容：{chunks:?}"
                );
            }
            other => panic!("应为 Update 块：{other:?}"),
        }
    }

    // ---- seek_sequence：三级容差与边界 ----

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn seek_exact_then_rstrip_then_trim() {
        let lines = owned(&["a", "b ", " c"]);
        assert_eq!(seek_sequence(&lines, &owned(&["b "]), 0, false), Some(1));
        assert_eq!(seek_sequence(&lines, &owned(&["b"]), 0, false), Some(1), "行尾空白容差");
        assert_eq!(seek_sequence(&lines, &owned(&["  c "]), 0, false), Some(2), "两端空白容差");
        assert_eq!(seek_sequence(&lines, &owned(&["zz"]), 0, false), None);
    }

    #[test]
    fn seek_respects_start_index() {
        let lines = owned(&["m", "m"]);
        assert_eq!(seek_sequence(&lines, &owned(&["m"]), 1, false), Some(1), "从 start 起找");
        assert_eq!(seek_sequence(&lines, &owned(&["m"]), 2, false), None, "越界起点不匹配");
    }

    #[test]
    fn seek_eof_anchors_at_end() {
        let lines = owned(&["x", "tail", "x"]);
        assert_eq!(seek_sequence(&lines, &owned(&["x"]), 0, true), Some(2), "eof 先锚定末尾");
        assert_eq!(seek_sequence(&lines, &owned(&["x"]), 0, false), Some(0), "非 eof 从头找");
    }

    #[test]
    fn seek_pattern_longer_than_lines_is_none() {
        let lines = owned(&["a"]);
        assert_eq!(seek_sequence(&lines, &owned(&["a", "b"]), 0, false), None);
        assert_eq!(seek_sequence(&lines, &[], 5, false), Some(5), "空模式为无操作匹配");
    }

    // ---- 029 M3：第四级 Unicode 标点/空格匹配容差 ----

    /// B1 单元层：28 个码点（行内位置，防被 trim 先行消除）与对应 ASCII 双向可
    /// 匹配；同组内部也可匹配。
    #[test]
    fn seek_level4_maps_all_unicode_punct_to_ascii() {
        let dashes = ['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}'];
        let singles = ['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];
        let doubles = ['\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}'];
        let spaces = [
            '\u{00A0}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}',
            '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}',
        ];
        let groups: [&[char]; 4] = [&dashes, &singles, &doubles, &spaces];
        let ascii_for = ['-', '\'', '"', ' '];
        for (gi, chars) in groups.iter().enumerate() {
            let expect = ascii_for[gi];
            for ch in *chars {
                // Unicode 源 ↔ ASCII 补丁；ASCII 源 ↔ Unicode 补丁（两侧都映射）
                let uni = format!("a{ch}b");
                let asc = format!("a{expect}b");
                assert_eq!(
                    seek_sequence(&owned(&[uni.as_str()]), &owned(&[asc.as_str()]), 0, false),
                    Some(0),
                    "源含 {ch:?} 应与补丁 {expect:?} 匹配"
                );
                assert_eq!(
                    seek_sequence(&owned(&[asc.as_str()]), &owned(&[uni.as_str()]), 0, false),
                    Some(0),
                    "补丁含 {ch:?} 应与源 ASCII {expect:?} 匹配"
                );
            }
        }
        // 同组内部：两种不同 Unicode 破折号都映射到 '-'
        assert_eq!(
            seek_sequence(&owned(&["a\u{2013}b"]), &owned(&["a\u{2012}b"]), 0, false),
            Some(0),
            "同映射组内部可匹配"
        );
    }

    /// B2 单元层：四级按完整搜索范围依次尝试——前面的宽松候选不得抢走后面的
    /// 精确/较严格匹配；第四级不越过 start、EOF 锚定仍生效。
    #[test]
    fn seek_level4_priority_start_and_eof() {
        // 行 0 仅第四级可匹配（em dash ↔ ASCII），行 1 精确匹配——精确优先
        let lines = owned(&["a-b", "a\u{2014}b"]);
        assert_eq!(
            seek_sequence(&lines, &owned(&["a\u{2014}b"]), 0, false),
            Some(1),
            "后面的精确匹配优先于前面的第四级宽松匹配"
        );
        // 无精确候选时第四级取最早候选
        let lines = owned(&["a-b", "c-d"]);
        assert_eq!(seek_sequence(&lines, &owned(&["a\u{2014}b"]), 0, false), Some(0));
        // start 起点不因第四级越过
        assert_eq!(seek_sequence(&lines, &owned(&["a\u{2014}b"]), 1, false), None);
        // EOF 锚定：末行第四级匹配
        let lines = owned(&["x", "tail", "a-b"]);
        assert_eq!(seek_sequence(&lines, &owned(&["a\u{2014}b"]), 0, true), Some(2));
        // 行尾空白容差（二级）先于第四级整行归一：两侧等价场景回归
        let lines = owned(&["a\u{00A0}"]);
        assert_eq!(seek_sequence(&lines, &owned(&["a "]), 0, false), Some(0));
    }

    /// B4 单元层：未映射标点、全角字母、大小写、组合字符、多个连续空格不匹配。
    #[test]
    fn seek_level4_keeps_unmapped_differences() {
        let cases: Vec<(&str, &str)> = vec![
            ("a〜b", "a~b"),            // U+301C 波浪线未映射
            ("ａｂ", "ab"),             // 全角字母不折叠
            ("Ab", "ab"),               // 大小写区分
            ("áb", "ab"),               // 组合字符不剥离
            ("a  b", "a b"),            // 连续空格不合并
            ("a\u{2026}b", "a.b"),      // 省略号未映射
        ];
        for (line, pat) in cases {
            assert_eq!(
                seek_sequence(&owned(&[line]), &owned(&[pat]), 0, false),
                None,
                "{line:?} 与 {pat:?} 不应匹配"
            );
        }
    }

    // ---- compute_replacements：哨兵重试与上下文切分 ----

    #[test]
    fn replacements_retry_without_trailing_empty_sentinel() {
        // 末尾空行是“最终换行”哨兵：去掉后重试
        let lines = owned(&["a", "b"]);
        let chunk = UpdateChunk {
            old_lines: owned(&["a", ""]),
            new_lines: owned(&["x", ""]),
            ..UpdateChunk::default()
        };
        let reps = compute_replacements(&lines, "f", &[chunk]).unwrap();
        assert_eq!(reps, vec![(0, 1, owned(&["x"]))]);
    }

    #[test]
    fn replacements_split_around_context_lines() {
        let lines = owned(&["pre", "keep", "mid", "tail", "post"]);
        let mut chunk = UpdateChunk::default();
        chunk.push_context_line("keep".into());
        chunk.old_lines.push("mid".into());
        chunk.new_lines.push("new".into());
        chunk.push_context_line("tail".into());
        let reps = compute_replacements(&lines, "f", &[chunk]).unwrap();
        assert_eq!(reps, vec![(2, 1, owned(&["new"]))], "上下文行保留为原始行");
    }

    #[test]
    fn change_context_seek_advances_search_start() {
        let lines = owned(&["head", "fn a", "old", "fn b", "old"]);
        let chunk_a = UpdateChunk {
            change_context: Some("fn a".into()),
            old_lines: owned(&["old"]),
            new_lines: owned(&["new-a"]),
            ..UpdateChunk::default()
        };
        let chunk_b = UpdateChunk {
            change_context: Some("fn b".into()),
            old_lines: owned(&["old"]),
            new_lines: owned(&["new-b"]),
            ..UpdateChunk::default()
        };
        let reps = compute_replacements(&lines, "f", &[chunk_a, chunk_b]).unwrap();
        assert_eq!(
            reps,
            vec![(2, 1, owned(&["new-a"])), (4, 1, owned(&["new-b"]))]
        );
    }

    // ---- SourceFile：行尾保持 ----

    #[test]
    fn source_file_preferred_ending_is_first_found() {
        assert_eq!(SourceFile::parse("a\r\nb\nc\n").preferred_ending, LineEnding::CrLf);
        assert_eq!(SourceFile::parse("a\nb\r\n").preferred_ending, LineEnding::Lf);
        assert_eq!(SourceFile::parse("a\rb\r").preferred_ending, LineEnding::Cr);
        assert_eq!(SourceFile::parse("abc").preferred_ending, LineEnding::Lf);
    }

    #[test]
    fn source_file_roundtrips_contents() {
        for content in ["a\nb\n", "a\r\nb\r\n", "a\rb\r", "x\ny", "", "no-newline"] {
            assert_eq!(SourceFile::parse(content).into_contents(), content);
        }
    }

    #[test]
    fn apply_replacements_keep_endings_and_complete_trailing() {
        // 约定风格 CRLF（首个换行）；替换行用 CRLF，未涉及行保持原样，
        // 无行尾的末行补齐
        let mut sf = SourceFile::parse("top\r\nmid\nend");
        sf.apply_replacements(&[(1, 1, vec!["MID".into()])]);
        assert_eq!(sf.into_contents(), "top\r\nMID\r\nend\r\n");
    }
    /// F2 最终验收：真实同步屏障下的窗口替换（确定性，不依赖计时窗口）。
    /// 子进程（本测试二进制 `--exact` 重入）在 Update 完成打开＋判型＋读入后、
    /// 写入前停在 [`f2_seam`]；控制端此刻把目标替换为 FIFO，再放行写入。
    /// 期望：写入绑定已判型 inode（原内容逐字节正确）、FIFO 未被读写、
    /// 清单与 state_known 正确。若实现回退为按路径重开，写打开 FIFO 将阻塞，
    /// 由看门狗捕获（专用副本变异已证）。
    #[test]
    fn f2_swap_barrier_binds_write_to_validated_inode() {
        use std::process::{Command, Stdio};
        // —— 子进程模式：启用接缝，经真实 apply_patch 入口执行补丁 ——
        if std::env::var_os("TCHAT_F2_SEAM_CHILD").is_some() {
            let dir: std::path::PathBuf = std::env::var("TCHAT_F2_SEAM_DIR").unwrap().into();
            f2_seam::arm();
            let patch_text = std::fs::read_to_string(dir.join("patch.txt")).unwrap();
            let r = apply_patch(&json!({ "patch": patch_text }));
            std::fs::write(dir.join("out.json"), serde_json::to_string(&r).unwrap()).unwrap();
            std::process::exit(0);
        }

        // 控制端：等待 reached（已判型＋读入完成、写前暂停）→ 执行替换 → 放行 → 看门狗收集
        let run_barrier = |name: &str,
                           patch_text: String,
                           swap: &dyn Fn(&std::path::Path)|
         -> serde_json::Value {
            let dir =
                std::env::temp_dir().join(format!("tchat-f2bar-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("patch.txt"), patch_text).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("apply_patch::tests::f2_swap_barrier_binds_write_to_validated_inode")
                .env("TCHAT_F2_SEAM_CHILD", "1")
                .env("TCHAT_F2_SEAM_DIR", &dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("子进程启动失败");
            let reached = dir.join("reached");
            let wait_dl = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while !reached.exists() {
                assert!(
                    std::time::Instant::now() < wait_dl,
                    "屏障未到达：未完成判型/读入，或接缝被移除"
                );
                if let Ok(Some(st)) = child.try_wait() {
                    panic!("子进程在屏障前退出：{st}");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            swap(&dir); // 确定性的写前时点：完成目标替换
            std::fs::write(dir.join("release"), b"").unwrap(); // 放行写入
            let dl = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                match child.try_wait() {
                    Ok(Some(st)) => {
                        assert!(st.success(), "子进程异常退出：{st}");
                        let text = std::fs::read_to_string(dir.join("out.json"))
                            .unwrap_or_else(|e| panic!("子进程未写结果：{e}"));
                        let _ = std::fs::remove_dir_all(&dir);
                        return serde_json::from_str(&text).unwrap();
                    }
                    Ok(None) if std::time::Instant::now() < dl => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("看门狗超时：写入按路径重开并阻塞在 FIFO（F2 回归）");
                    }
                }
            }
        };

        let root = std::env::temp_dir().join(format!("tchat-f2bar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let expect_new = "R1\nR2\nR3\nR4\nR5\n";

        // 变体一：直接目标。屏障时点把 victim 改名为 orig，FIFO 占位 victim 路径。
        let victim = root.join("victim.txt");
        let orig = root.join("orig.txt");
        std::fs::write(&victim, "a\nb\nc\nd\ne\n").unwrap();
        let patch1 = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-a\n-b\n-c\n-d\n-e\n+R1\n+R2\n+R3\n+R4\n+R5\n*** End Patch",
            victim.display()
        );
        let (v, o) = (victim.clone(), orig.clone());
        let r1 = run_barrier("v1", patch1, &move |_| {
            std::fs::rename(&v, &o).unwrap();
            let c = std::ffi::CString::new(v.as_os_str().as_encoded_bytes().to_vec()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        });
        assert_eq!(r1["ok"], json!(true), "写入须绑定已判型 inode：{r1}");
        assert_eq!(r1["state_known"], json!(true), "{r1}");
        assert_eq!(
            r1["changed_files"],
            json!([{ "path": victim.display().to_string(), "action": "update" }]),
            "{r1}"
        );
        assert_eq!(std::fs::read(&orig).unwrap(), expect_new.as_bytes(), "原 inode 内容逐字节正确");
        assert!(
            std::fs::symlink_metadata(&victim).unwrap().file_type().is_fifo(),
            "占位 FIFO 未被读写（仍为 FIFO）"
        );

        // 变体二：链接目标。屏障时点以原子 rename 把链接本体替换为 FIFO。
        let realf = root.join("real.txt");
        let link = root.join("link.txt");
        std::fs::write(&realf, "a\nb\nc\nd\ne\n").unwrap();
        std::os::unix::fs::symlink(&realf, &link).unwrap();
        let patch2 = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-a\n-b\n-c\n-d\n-e\n+R1\n+R2\n+R3\n+R4\n+R5\n*** End Patch",
            link.display()
        );
        let (l, f) = (link.clone(), root.join("fifo2.txt"));
        let r2 = run_barrier("v2", patch2, &move |_| {
            let c = std::ffi::CString::new(f.as_os_str().as_encoded_bytes().to_vec()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
            std::fs::rename(&f, &l).unwrap(); // 原子替换链接本体，无中间状态
        });
        assert_eq!(r2["ok"], json!(true), "链接目标经 fd 绑定写入原 inode：{r2}");
        assert_eq!(r2["state_known"], json!(true), "{r2}");
        assert_eq!(
            r2["changed_files"],
            json!([{ "path": link.display().to_string(), "action": "update" }]),
            "{r2}"
        );
        assert_eq!(std::fs::read(&realf).unwrap(), expect_new.as_bytes(), "原 inode 内容逐字节正确");
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_fifo(),
            "重指向后的 FIFO 未被读写（仍为 FIFO）"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 029：Move 窗口测试（子进程屏障模式，与 f2_swap_barrier 同构）----

    /// 子进程模式共用入口：启用接缝→执行 patch.txt→写 out.json→退出。
    /// 非子进程返回 false，用例继续走控制端逻辑。
    fn move_seam_child_mode() -> bool {
        if std::env::var_os("TCHAT_MOVE_SEAM_CHILD").is_none() {
            return false;
        }
        let dir: std::path::PathBuf = std::env::var("TCHAT_MOVE_SEAM_DIR").unwrap().into();
        move_seam::arm();
        let patch_text = std::fs::read_to_string(dir.join("patch.txt")).unwrap();
        let r = apply_patch(&json!({ "patch": patch_text }));
        std::fs::write(dir.join("out.json"), serde_json::to_string(&r).unwrap()).unwrap();
        std::process::exit(0);
    }

    /// 启动重入本测试二进制的接缝子进程。
    fn spawn_move_seam_child(test_name: &str, dir: &std::path::Path) -> std::process::Child {
        use std::process::{Command, Stdio};
        Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test_name)
            .env("TCHAT_MOVE_SEAM_CHILD", "1")
            .env("TCHAT_MOVE_SEAM_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("子进程启动失败")
    }

    /// 等待具名屏障到达（子进程提前退出即失败）。
    fn wait_move_barrier(
        dir: &std::path::Path,
        name: &str,
        child: &mut std::process::Child,
    ) {
        let reached = dir.join(format!("reached-{name}"));
        let wait_dl = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !reached.exists() {
            assert!(
                std::time::Instant::now() < wait_dl,
                "屏障 {name} 未到达：实现未到达该时点，或接缝被移除"
            );
            if let Ok(Some(st)) = child.try_wait() {
                panic!("子进程在屏障 {name} 前退出：{st}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn release_move_barrier(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(format!("release-{name}")), b"").unwrap();
    }

    /// 收集子进程结果 JSON。
    fn collect_move_child(
        dir: &std::path::Path,
        child: std::process::Child,
    ) -> serde_json::Value {
        let out = child.wait_with_output().expect("等待子进程失败");
        assert!(out.status.success(), "子进程异常退出：{out:?}");
        let text = std::fs::read_to_string(dir.join("out.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn write_move_patch(dir: &std::path::Path, src: &Path, dst: &Path) {
        std::fs::write(
            dir.join("patch.txt"),
            format!(
                "*** Begin Patch\n*** Update File: {s}\n*** Move to: {d}\n*** End Patch",
                s = src.display(),
                d = dst.display()
            ),
        )
        .unwrap();
    }

    /// 审查 #8 回归：fstat 与读取之间源被并发截断——短读与元数据尺寸不符，
    /// 必须保守拒绝（此前只拦「读多」方向，残片曾被当完整内容移动并删源）。
    /// before_read 屏障给出确定性截断时点。
    #[test]
    fn move_concurrent_truncation_short_read_rejected() {
        if move_seam_child_mode() {
            unreachable!("子进程已在 move_seam_child_mode 内退出");
        }
        let dir = std::env::temp_dir().join(format!("tchat-movetrunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        std::fs::write(&src, "A\n".repeat(50_000)).unwrap(); // ~100 KiB
        write_move_patch(&dir, &src, &dst);
        let mut child = spawn_move_seam_child(
            "apply_patch::tests::move_concurrent_truncation_short_read_rejected",
            &dir,
        );
        wait_move_barrier(&dir, "before_read", &mut child);
        // 确定性的读入窗口内截断：只剩 10 行
        let truncated = "B\n".repeat(10);
        std::fs::write(&src, &truncated).unwrap();
        release_move_barrier(&dir, "before_read");
        let r = collect_move_child(&dir, child);
        assert_eq!(r["ok"], json!(false), "短读必须拒绝：{r}");
        assert_eq!(r["state_known"], json!(true), "校验型失败：{r}");
        assert_eq!(r["changed_files"], json!([]), "不得移动残片：{r}");
        let err = r["error"].as_str().unwrap();
        assert!(err.contains("截断") || err.contains("不符"), "错误标明读入不一致：{err}");
        assert!(!dst.exists(), "不得创建目标");
        assert_eq!(std::fs::read_to_string(&src).unwrap(), truncated, "截断后的源原样保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 审查 #10 奇偶断言：logical_lines（零分配计数）与 SourceFile::parse 的
    /// 行切分在任何输入上同源——line_ranges 原语由构造保证，此测试钉住不回归。
    #[test]
    fn logical_lines_parity_with_source_file_parse() {
        for s in [
            "", "a", "a\n", "a\r\nb\r\n", "a\rb\r", "x\ny",
            "no-newline", "a\r\nb\nc", "\r\n\r\n", "x\r\r\ry",
            "l\nl\r\nl\nl\r\nl\nl\r\nt",
        ] {
            assert_eq!(
                logical_lines(s),
                SourceFile::parse(s).lines.len(),
                "计数与解析必须同源：{s:?}"
            );
        }
    }

    /// correctness 缺口钉扎：eof=true 只锚定末尾、无全范围回退——pattern 仅在
    /// 早处匹配时应返回 None（自 004 起的既有语义，防未来引入回退扫描）。
    #[test]
    fn seek_eof_anchor_has_no_full_range_fallback() {
        let lines = owned(&["tail", "x"]);
        assert_eq!(
            seek_sequence(&lines, &owned(&["tail"]), 0, true),
            None,
            "eof 锚定不得回退到早位匹配"
        );
    }

    /// 029 A5 竞态：预检通过后、排他创建前目标被抢占创建——`create_new` 必须以
    /// EEXIST 拒绝（不覆盖抢占者），抢占者字节与源均不变；未产生本次变更，
    /// 状态已知。
    #[test]
    fn move_target_race_rejected_exclusively() {
        if move_seam_child_mode() {
            unreachable!("子进程已在 move_seam_child_mode 内退出");
        }
        let dir = std::env::temp_dir().join(format!("tchat-moverace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        std::fs::write(&src, "S\n").unwrap();
        write_move_patch(&dir, &src, &dst);
        let mut child = spawn_move_seam_child(
            "apply_patch::tests::move_target_race_rejected_exclusively",
            &dir,
        );
        release_move_barrier(&dir, "before_read"); // 读入阶段无竞态需求，直接放行
        wait_move_barrier(&dir, "before_create", &mut child);
        // 确定性的创建前时点：抢占创建目标
        std::fs::write(&dst, "RACER\n").unwrap();
        release_move_barrier(&dir, "before_create");
        let r = collect_move_child(&dir, child);
        assert_eq!(r["ok"], json!(false), "目标被抢占必须失败：{r}");
        assert_eq!(r["state_known"], json!(true), "仅创建竞争且未产生本次变更：{r}");
        assert_eq!(r["changed_files"], json!([]), "{r}");
        assert!(r["error"].as_str().unwrap().contains("已存在"), "{r}");
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "RACER\n",
            "抢占者字节不变（未被覆盖）"
        );
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "S\n", "源不变");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 029 A7b：删源前源对象更换——真实同步屏障下的确定性替换。子进程在 Move
    /// 完成目标写入并记录 add 后、删源复核前停在 `before_unlink`；控制端此刻把
    /// 源改名挪走并在原路径放入替换文件，再放行。期望：ok=false、清单保留目标
    /// add、不记源 delete、state_known=false；替换对象与被挪走的原文件均不被删。
    #[test]
    fn move_source_swap_detected_before_unlink() {
        if move_seam_child_mode() {
            unreachable!("子进程已在 move_seam_child_mode 内退出");
        }
        let dir = std::env::temp_dir().join(format!("tchat-moveswap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        std::fs::write(&src, "ORIGINAL\n").unwrap();
        write_move_patch(&dir, &src, &dst);
        let mut child = spawn_move_seam_child(
            "apply_patch::tests::move_source_swap_detected_before_unlink",
            &dir,
        );
        // 读入与创建前屏障均无需竞态：直接放行（子进程随后写入目标并记录 add）
        release_move_barrier(&dir, "before_read");
        release_move_barrier(&dir, "before_create");
        wait_move_barrier(&dir, "before_unlink", &mut child);
        // 确定性的删源前时点：挪走原文件，原路径放入替换对象
        let moved_away = dir.join("moved-away.txt");
        std::fs::rename(&src, &moved_away).unwrap();
        std::fs::write(&src, "IMPOSTOR\n").unwrap();
        release_move_barrier(&dir, "before_unlink");
        let r = collect_move_child(&dir, child);
        assert_eq!(r["ok"], json!(false), "源更换必须失败：{r}");
        assert_eq!(r["state_known"], json!(false), "{r}");
        assert_eq!(
            r["changed_files"],
            json!([{ "path": dst.display().to_string(), "action": "add" }]),
            "清单保留已完整写入的目标 add、不记源 delete：{r}"
        );
        let err = r["error"].as_str().unwrap();
        assert!(err.contains("更换"), "错误标明源对象更换：{err}");
        // 磁盘事实：目标＝移动内容；替换对象与被挪走的原文件都在
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "ORIGINAL\n", "目标含完整移动内容");
        assert_eq!(
            std::fs::read_to_string(&src).unwrap(),
            "IMPOSTOR\n",
            "可检测的替换对象不得被删"
        );
        assert_eq!(
            std::fs::read_to_string(&moved_away).unwrap(),
            "ORIGINAL\n",
            "被挪走的原文件保留（用户重读后决定下一步）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- 012：展示差异捕获（apply_patch_with_display——真实前后内容）----

    /// 临时目录助手：测试内绝对路径补丁（apply 以进程 cwd 为 base，绝对路径直达）。
    fn disp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tchat-012-disp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn display_changes_capture_add_update_delete_real_content() {
        let dir = disp_dir("aud");
        let added = dir.join("new.txt");
        let updated = dir.join("upd.txt");
        let deleted = dir.join("del.txt");
        std::fs::write(&updated, "keep-a\nold-1\nold-2\nkeep-b\n").unwrap();
        std::fs::write(&deleted, "del-line-1\ndel-line-2\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Add File: {a}\n+added-line-1\n+added-line-2\n*** Update File: {u}\n@@\n-old-1\n-old-2\n+new-1\n*** Delete File: {d}\n*** End Patch",
            a = added.display(),
            u = updated.display(),
            d = deleted.display(),
        );
        let (r, changes) = apply_patch_with_display(&json!({ "patch": patch }));
        let r = serde_json::to_value(&r).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(changes.len(), 3, "每文件一条展示差异：{changes:?}");

        // Add：真实新全文
        match &changes[0].kind {
            crate::prompt::PatchChangeKind::Add { content } => {
                assert_eq!(content, "added-line-1\nadded-line-2\n");
            }
            other => panic!("Add 形状：{other:?}"),
        }
        assert_eq!(changes[0].path, added.display().to_string());

        // Update：统一 diff 来自真实前后缓冲——含 -old/+new；可再被 diffy 解析
        match &changes[1].kind {
            crate::prompt::PatchChangeKind::Update { diff } => {
                assert!(diff.contains("-old-1"), "删除行：{diff}");
                assert!(diff.contains("-old-2"), "删除行：{diff}");
                assert!(diff.contains("+new-1"), "新增行：{diff}");
                assert!(diffy::Patch::from_str(diff).is_ok(), "统一 diff 可解析：{diff}");
            }
            other => panic!("Update 形状：{other:?}"),
        }
        // 磁盘事实交叉验证：diff 描述的对象＝真实最终内容
        assert_eq!(
            std::fs::read_to_string(&updated).unwrap(),
            "keep-a\nnew-1\nkeep-b\n",
            "磁盘真实结果（diff 描述的对象）"
        );

        // Delete：真实旧全文（删除前读入）
        match &changes[2].kind {
            crate::prompt::PatchChangeKind::Delete { content } => {
                assert_eq!(content.as_deref(), Some("del-line-1\ndel-line-2\n"));
            }
            other => panic!("Delete 形状：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_changes_return_partial_prefix_on_failure() {
        let dir = disp_dir("partial");
        let ok_file = dir.join("ok.txt");
        let patch = format!(
            "*** Begin Patch\n*** Add File: {a}\n+first\n*** Update File: {missing}\n@@\n-x\n+y\n*** End Patch",
            a = ok_file.display(),
            missing = dir.join("missing.txt").display(),
        );
        let (r, changes) = apply_patch_with_display(&json!({ "patch": patch }));
        let r = serde_json::to_value(&r).unwrap();
        assert_eq!(r["ok"], json!(false), "第二个目标不存在→失败：{r}");
        assert_eq!(changes.len(), 1, "仅已生效前缀的差异：{changes:?}");
        assert_eq!(changes[0].path, ok_file.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_delete_oversize_old_content_degrades_to_none() {
        let dir = disp_dir("big");
        let big = dir.join("big.txt");
        // 超过展示读入上限（MAX_FILE_BYTES）：删除仍成功，旧内容降级 None（不伪造）
        std::fs::write(&big, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Delete File: {b}\n*** End Patch",
            b = big.display(),
        );
        let (r, changes) = apply_patch_with_display(&json!({ "patch": patch }));
        let r = serde_json::to_value(&r).unwrap();
        assert_eq!(r["ok"], json!(true), "删除本身不受展示读入上限影响：{r}");
        match &changes[0].kind {
            crate::prompt::PatchChangeKind::Delete { content } => {
                assert!(content.is_none(), "超限旧文＝明确降级 None：{changes:?}");
            }
            other => panic!("Delete 形状：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_json_unchanged_by_display_capture() {
        // 012 模型契约：模型面结果 JSON 不因展示捕获而改变、不含展示字段。
        // 两次调用各用独立目录（Add 不覆盖），路径归一后逐字节比对。
        let norm = |dir: &std::path::Path, r: &ApplyPatchResult| -> String {
            serde_json::to_string(r)
                .unwrap()
                .replace(dir.display().to_string().as_str(), "<DIR>")
        };
        let dir_a = disp_dir("contract-a");
        let pa = format!(
            "*** Begin Patch\n*** Add File: {t}\n+body\n*** End Patch",
            t = dir_a.join("c.txt").display(),
        );
        let with = apply_patch_with_display(&json!({ "patch": pa }));
        let dir_b = disp_dir("contract-b");
        let pb = format!(
            "*** Begin Patch\n*** Add File: {t}\n+body\n*** End Patch",
            t = dir_b.join("c.txt").display(),
        );
        let plain = apply_patch(&json!({ "patch": pb }));
        assert_eq!(norm(&dir_a, &with.0), norm(&dir_b, &plain));
        let v: serde_json::Value = serde_json::to_value(&plain).unwrap();
        assert!(v.get("changes").is_none() && v.get("display").is_none(), "结果 JSON 不含展示字段：{v}");
        assert!(with.1.len() == 1, "展示差异并行返回：{:?}", with.1);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

}