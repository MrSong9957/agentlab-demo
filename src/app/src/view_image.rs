//! 019：view_image 工具——读取本地图片文件、按上游 HIGH_DETAIL 策略准备并产出
//! data URL 载荷。函数级移植 openai/codex@`d138398`（Apache-2.0）的
//! `codex-rs/utils/image/src/lib.rs`：内容识别与解码、2048/32px/2500 patches
//! 缩放（`load_for_prompt_bytes_uncached`→[`prepare_image`]、
//! `prompt_image_output_dimensions_for_limits`→[`high_detail_output_dimensions`]）、
//! PNG/JPEG/WebP 源字节保留（`can_preserve_source_bytes`）、RGB ICC 与 EXIF 元数据
//! 处理（`encode_image`/`apply_image_metadata`）、GIF 转 PNG、
//! `data_url_from_bytes`；省去上游缓存、附件上传、data URL 输入解析与其他
//! detail 模式。本地固定资源边界（§3.1，非上游或提供方限额）：输入文件 20 MiB、
//! 解码前宽高各 16384、解码器 64 MiB 分配预算、准备后图片 4 MiB。

use crate::prompt::ToolResultImage;
use base64::Engine;
use serde_json::Value;
use std::io::Cursor;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// 输入文件字节上限（§3.1）。
pub const MAX_INPUT_FILE_BYTES: usize = 20 * 1024 * 1024;
/// 解码前单边像素上限（宽、高各自，§3.1）。
pub const MAX_DECODE_DIMENSION: u32 = 16384;
/// 解码器分配预算（§3.1）。
pub const MAX_DECODER_ALLOC_BYTES: u64 = 64 * 1024 * 1024;
/// 准备后图片字节上限（§3.1）；超限返回工具错误，不循环降质。
pub const MAX_PREPARED_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// 上游 `PROMPT_IMAGE_PATCH_SIZE`。
const PROMPT_IMAGE_PATCH_SIZE: u32 = 32;
/// 上游 `PromptImageMode::HIGH_DETAIL` 的两条限制。
const HIGH_DETAIL_MAX_DIMENSION: u32 = 2048;
const HIGH_DETAIL_MAX_PATCHES: usize = 2500;

/// 准备后的图片（上游 `EncodedImage` 的本地形态；无缓存共享故用 Vec）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
}

/// 上游 `data_url_from_bytes`：包装为 data URL（本层不解码校验）。
pub fn data_url_from_bytes(mime: &str, bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{encoded}")
}

/// 工具执行结果：文本摘要（入史 `ToolResult.content`）＋可选图片载荷。
pub struct ViewImageOutcome {
    pub summary: String,
    pub image: Option<ToolResultImage>,
}

fn ok_summary(path: &Path, encoded: &EncodedImage) -> String {
    serde_json::json!({
        "ok": true,
        "path": path.display().to_string(),
        "source_width": encoded.source_width,
        "source_height": encoded.source_height,
        "width": encoded.width,
        "height": encoded.height,
    })
    .to_string()
}

fn error_summary(error: &str) -> String {
    serde_json::json!({ "ok": false, "error": error }).to_string()
}

/// 能力关闭（text-only／未知）时的结果：不读取文件（§3.2——声明与执行两层关闭）。
/// 以错误摘要构造结果（031 编码失败等调用方侧失败；不读取文件）。
pub fn error_outcome(error: &str) -> ViewImageOutcome {
    ViewImageOutcome {
        summary: error_summary(error),
        image: None,
    }
}

pub fn unsupported_outcome() -> ViewImageOutcome {
    ViewImageOutcome {
        summary: error_summary(
            "view_image 不可用：当前模型未确认支持图片输入（input_modalities 缺少 image）",
        ),
        image: None,
    }
}

/// view_image 参数（上游 `ViewImageArgs` 最小子集：仅必填 `path`）。
fn parse_arguments(arguments: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("工具参数不是有效 JSON：{e}"))?;
    if !v.is_object() {
        return Err("工具参数必须是 JSON 对象".into());
    }
    let Some(path) = v.get("path") else {
        return Err("缺少必填字段 path".into());
    };
    let Some(path) = path.as_str() else {
        return Err("path 必须是非空字符串".into());
    };
    if path.trim().is_empty() {
        return Err("path 必须是非空字符串".into());
    }
    Ok(path.to_string())
}

/// 执行一次 view_image 调用（§3.1 契约）：参数解析 → 路径解析（相对＝会话
/// 工作目录，绝对＝原义，不作 shell 展开）→ 有界读取 → 图片准备 → 摘要与载荷。
/// 取消检查点：读取前、读取后、结果提交前（解码为有界同步处理，途中取消在
/// 下一检查点生效）。
pub fn execute(arguments: &str, cwd: &Path, cancel: Option<&AtomicBool>) -> ViewImageOutcome {
    let run = || -> Result<(PathBuf, EncodedImage), String> {
        let path = parse_arguments(arguments)?;
        let resolved = resolve_path(cwd, &path);
        if cancelled(cancel) {
            return Err("用户已取消：本调用未完成".into());
        }
        let encoded = prepare_from_path(&resolved, cancel)?;
        if cancelled(cancel) {
            return Err("用户已取消：本调用未完成".into());
        }
        Ok((resolved, encoded))
    };
    match run() {
        Ok((resolved, encoded)) => ViewImageOutcome {
            summary: ok_summary(&resolved, &encoded),
            image: Some(ToolResultImage {
                image_url: data_url_from_bytes(&encoded.mime, &encoded.bytes),
                detail: "high".into(),
            }),
        },
        Err(error) => ViewImageOutcome { summary: error_summary(&error), image: None },
    }
}

/// 从已解析路径准备单张图片（020：与工具共用核心——普通文件检查、
/// 有界读取、HIGH_DETAIL 准备与 4 MiB 上限；无参数解析与取消点归调用方组合）。
pub(crate) fn prepare_from_path(
    resolved: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<EncodedImage, String> {
    // 读取前检查点（A12：能力门通过后的第一个取消点）
    if cancelled(cancel) {
        return Err("用户已取消：本调用未完成".into());
    }
    let mut file = open_regular_file(resolved)?;
    let bytes = read_bounded(&mut file, resolved)?;
    if cancelled(cancel) {
        return Err("用户已取消：本调用未完成".into());
    }
    prepare_image(resolved, bytes)
}

/// 031（§1.6 接入面）：受控内部图片准备入口——模型指定路径的**打开与读取
/// 发生在 031 沙箱子进程**（本二进制 `--internal-view-image <path>`），主进程
/// 只接收有界编码图片（JSON on stdout），不因格式门存在而先读取越权文件。
/// 沙箱不可用＝显式错误（fail-closed，不回退进程内读取）。
pub fn execute_sandboxed(
    arguments: &str,
    cwd: &Path,
    cancel: Option<&AtomicBool>,
    snapshot: &crate::sandbox::PolicySnapshot,
) -> ViewImageOutcome {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from(
        "/proc/self/exe",
    ));
    execute_sandboxed_with_exe(arguments, cwd, cancel, snapshot, &exe)
}

/// [`execute_sandboxed`] 的显式二进制变体（测试注入 CARGO_BIN_EXE——测试
/// 进程的 current_exe 是 harness，内部入口模式在真实二进制上）。
pub fn execute_sandboxed_with_exe(
    arguments: &str,
    cwd: &Path,
    cancel: Option<&AtomicBool>,
    snapshot: &crate::sandbox::PolicySnapshot,
    exe: &Path,
) -> ViewImageOutcome {
    let run = || -> Result<(PathBuf, EncodedImage), String> {
        let path = parse_arguments(arguments)?;
        let resolved = resolve_path(cwd, &path);
        if cancelled(cancel) {
            return Err("用户已取消：本调用未完成".into());
        }
        let launch = crate::sandbox::permission_launch_argv(
            vec![
                exe.to_string_lossy().into_owned(),
                "--internal-view-image".to_string(),
                resolved.to_string_lossy().into_owned(),
            ],
            snapshot,
            cwd,
        )
        .map_err(|e| format!("031 沙箱不可用：{e}"))?;
        use std::io::Write;
        use std::process::Stdio;
        let mut cmd = std::process::Command::new(&launch.program);
        cmd.args(&launch.args);
        cmd.env_clear();
        cmd.envs(launch.env.iter().cloned());
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                match launch.seccomp.as_ref() {
                    Some(g) => {
                        let fd = g.raw();
                        cmd.pre_exec(move || {
                            crate::sandbox::close_inherited_fds_except_cloexec(fd)
                        });
                    }
                    None => {
                        cmd.pre_exec(crate::sandbox::close_inherited_fds_mark_cloexec);
                    }
                }
            }
        }
        // seccomp fd 守卫存活到 spawn 完成（memfd 非 CLOEXEC 穿越 execve）。
        let child = cmd
            .spawn()
            .map_err(|e| format!("沙箱图片准备启动失败：{e}"))?;
        drop(launch.seccomp);
        let mut child = child;
        // 审查 #27（§1.6 逐启动确认）：载荷确在新 PID ns
        crate::sandbox::verify_child_pid_namespace(child.id())
            .map_err(|e| format!("沙箱图片准备隔离验证失败：{e}"))?;
        let out = child
            .wait_with_output()
            .map_err(|e| format!("沙箱图片准备等待失败：{e}"))?;
        if !out.status.success() {
            let tail: String = {
                let all = String::from_utf8_lossy(&out.stderr);
                let skip = all.chars().count().saturating_sub(200);
                all.chars().skip(skip).collect()
            };
            return Err(format!("沙箱图片准备失败（exit {:?}）：{tail}", out.status.code()));
        }
        // 有界接收：编码图片上限即进程内同款 4 MiB＋元数据余量
        if out.stdout.len() > MAX_PREPARED_IMAGE_BYTES + 1024 {
            return Err("沙箱图片结果超过有界上限".to_string());
        }
        let encoded: EncodedImage = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("沙箱图片结果解析失败：{e}"))?;
        if cancelled(cancel) {
            return Err("用户已取消：本调用未完成".into());
        }
        Ok((resolved, encoded))
    };
    match run() {
        Ok((resolved, encoded)) => ViewImageOutcome {
            summary: ok_summary(&resolved, &encoded),
            image: Some(ToolResultImage {
                image_url: data_url_from_bytes(&encoded.mime, &encoded.bytes),
                detail: "high".into(),
            }),
        },
        Err(error) => ViewImageOutcome { summary: error_summary(&error), image: None },
    }
}

/// `--internal-view-image <path>` 子进程入口：读取并准备单张图片，编码 JSON
/// 写 stdout（仅由 [`execute_sandboxed`] 经 031 沙箱包装调用）。返回进程退出码。
pub fn run_internal_view_image() -> i32 {
    let Some(path) = std::env::args().nth(2) else {
        eprintln!("internal-view-image：缺少路径参数");
        return 2;
    };
    match prepare_from_path(Path::new(&path), None) {
        Ok(encoded) => {
            use std::io::Write;
            let mut ok = false;
            if let Ok(json) = serde_json::to_vec(&encoded) {
                ok = std::io::stdout().write_all(&json).is_ok();
            }
            if ok {
                0
            } else {
                2
            }
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// 020 §2.1：用户附件提交准备——能力检查先于任何文件打开（A11），随后逐图
/// 读取并准备（正式提交时才读：入队不冻结）。返回准备后的 data URL 列表
///（顺序即附件顺序）。任一失败即整体失败（消息不入史，由调用方恢复草稿）。
pub fn prepare_user_images(
    paths: &[PathBuf],
    image_input_capable: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<String>, String> {
    // A12：能力门→逐图准备（读取前/后取消检查点；准备取消＝消息不入史恢复草稿）
    if !image_input_capable {
        return Err(
            "当前模型未确认支持图片输入（input_modalities 缺少 image）：已拒绝附加图片"
                .into(),
        );
    }
    let mut urls = Vec::with_capacity(paths.len());
    for path in paths {
        let encoded = prepare_from_path(path, cancel)?;
        urls.push(data_url_from_bytes(&encoded.mime, &encoded.bytes));
    }
    Ok(urls)
}

/// 020：无 composer 场景（CLI `-i`）的用户消息合成——按图片数生成前置
/// 结构化标记 `[Image #N]` 与对应 `text_elements` 区间；文本非空时以空格
/// 接续在标记之后；无图返回原文与空元素。
pub fn compose_cli_user_message(
    text: &str,
    image_count: usize,
) -> (String, Vec<codex_protocol::user_input::TextElement>) {
    use codex_protocol::user_input::{ByteRange, TextElement};
    if image_count == 0 {
        return (text.to_string(), Vec::new());
    }
    let mut content = String::new();
    let mut elements = Vec::with_capacity(image_count);
    for n in 1..=image_count {
        let start = content.len();
        let marker = format!("[Image #{n}]");
        content.push_str(&marker);
        elements.push(TextElement::new(
            ByteRange { start, end: content.len() },
            Some(marker),
        ));
        if n < image_count {
            content.push(' ');
        }
    }
    if !text.trim().is_empty() {
        content.push(' ');
        content.push_str(text);
    }
    (content, elements)
}

/// 相对路径以会话工作目录解析；绝对路径按原义使用。
fn resolve_path(cwd: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// view_image 工具定义（Chat Completions function 形态；上游三选项全 false 的
/// 基础参数面：仅必填 `path`，strict:false、additionalProperties:false 同声明）。
pub fn tool_definition() -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "view_image",
            "description": "View a local image file from the filesystem when visual inspection is needed. Use this for images already available on disk.",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Local filesystem path to an image file."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }
    })
}

/// 上游 `load_for_prompt_bytes_uncached` 的本地移植（无缓存路径），固定
/// HIGH_DETAIL 模式并施加本地解码限额；错误消息为本地中文描述。
pub(crate) fn prepare_image(path: &Path, file_bytes: Vec<u8>) -> Result<EncodedImage, String> {
    let decode_err = |e: image::ImageError| {
        format!(
            "图片解码失败（{}）：{e}（宽/高各不超过 {MAX_DECODE_DIMENSION} 像素，解码器分配预算 {} MiB）",
            path.display(),
            MAX_DECODER_ALLOC_BYTES / 1024 / 1024
        )
    };
    let guessed_format = image::guess_format(&file_bytes)
        .map_err(|_| format!("无法识别图片内容（{}）：不是受支持的图片格式", path.display()))?;
    let format = match guessed_format {
        image::ImageFormat::Png => Some(image::ImageFormat::Png),
        image::ImageFormat::Jpeg => Some(image::ImageFormat::Jpeg),
        image::ImageFormat::Gif => Some(image::ImageFormat::Gif),
        image::ImageFormat::WebP => Some(image::ImageFormat::WebP),
        _ => None,
    };

    let mut reader = image::ImageReader::with_format(Cursor::new(&file_bytes), guessed_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODER_ALLOC_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(decode_err)?;
    use image::ImageDecoder;
    // 与上游一致：只保留对忠实重渲染最重要的色彩（仅 RGB ICC）与 EXIF 元数据。
    let icc_profile = decoder
        .icc_profile()
        .ok()
        .flatten()
        .filter(|profile| profile.get(16..20) == Some(b"RGB "));
    let exif = decoder.exif_metadata().ok().flatten();
    let dynamic = image::DynamicImage::from_decoder(decoder).map_err(decode_err)?;

    use image::GenericImageView;
    let (width, height) = dynamic.dimensions();

    let (target_width, target_height) = high_detail_output_dimensions(width, height);
    let encoded = if (target_width, target_height) != (width, height) {
        let resized = dynamic
            .resize_exact(target_width, target_height, image::imageops::FilterType::Triangle);
        let target_format = format
            .filter(|f| can_preserve_source_bytes(*f))
            .unwrap_or(image::ImageFormat::Png);
        let (bytes, output_format) = encode_image(&resized, target_format, icc_profile, exif)?;
        EncodedImage {
            bytes,
            mime: format_to_mime(output_format),
            source_width: width,
            source_height: height,
            width: target_width,
            height: target_height,
        }
    } else if let Some(format) = format.filter(|f| can_preserve_source_bytes(*f)) {
        // 无需缩放且格式可安全透传：保留源字节（上游小图行为）
        EncodedImage {
            bytes: file_bytes,
            mime: format_to_mime(format),
            source_width: width,
            source_height: height,
            width,
            height,
        }
    } else {
        let (bytes, output_format) =
            encode_image(&dynamic, image::ImageFormat::Png, icc_profile, exif)?;
        EncodedImage {
            bytes,
            mime: format_to_mime(output_format),
            source_width: width,
            source_height: height,
            width,
            height,
        }
    };
    // §3.1：准备后字节上限——超限即错误，不循环降低质量
    if encoded.bytes.len() > MAX_PREPARED_IMAGE_BYTES {
        return Err(format!(
            "准备后的图片 {} 字节超过 {} MiB 上限，已拒绝（不循环降低质量）",
            encoded.bytes.len(),
            MAX_PREPARED_IMAGE_BYTES / 1024 / 1024
        ));
    }
    Ok(encoded)
}

/// 上游 `prompt_image_output_dimensions_for_limits`（HIGH_DETAIL 固定值）：
/// 保持比例、不放大；先按最大边 2048 缩，再按 patch 面积预算（32px、2500 个）
/// 向下取整缩放。
fn high_detail_output_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    if dimensions_fit(width, height) {
        return (width, height);
    }

    let max_dimension_scale =
        (f64::from(HIGH_DETAIL_MAX_DIMENSION) / f64::from(width.max(height))).min(1.0);
    let width = ((f64::from(width) * max_dimension_scale).round() as u32).max(1);
    let height = ((f64::from(height) * max_dimension_scale).round() as u32).max(1);
    if dimensions_fit(width, height) {
        return (width, height);
    }

    let width_f64 = f64::from(width);
    let height_f64 = f64::from(height);
    let patch_size = f64::from(PROMPT_IMAGE_PATCH_SIZE);
    let mut scale =
        (patch_size * patch_size * HIGH_DETAIL_MAX_PATCHES as f64 / width_f64 / height_f64).sqrt();
    // 与 Responses patch 预算算法一致：按面积缩放后把缩放的 patch 网格向下取整，
    // 保证整数输出尺寸仍在预算内。
    let scaled_patches_wide = width_f64 * scale / patch_size;
    let scaled_patches_high = height_f64 * scale / patch_size;
    scale *= (scaled_patches_wide.floor() / scaled_patches_wide)
        .min(scaled_patches_high.floor() / scaled_patches_high);

    (
        ((width_f64 * scale).floor() as u32).max(1),
        ((height_f64 * scale).floor() as u32).max(1),
    )
}

/// 上游 `prompt_image_dimensions_fit`（HIGH_DETAIL 固定值）。
fn dimensions_fit(width: u32, height: u32) -> bool {
    let patches_wide = width.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patches_high = height.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patch_count = u64::from(patches_wide) * u64::from(patches_high);
    width <= HIGH_DETAIL_MAX_DIMENSION
        && height <= HIGH_DETAIL_MAX_DIMENSION
        && patch_count <= HIGH_DETAIL_MAX_PATCHES as u64
}

/// 上游 `can_preserve_source_bytes`：仅 PNG/JPEG/WebP 保留源字节。
fn can_preserve_source_bytes(format: image::ImageFormat) -> bool {
    matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    )
}

/// 上游 `encode_image`＋`apply_image_metadata`：PNG/JPEG(质量 85)/WebP 无损
/// 再编码，尽力保留 RGB ICC 与 EXIF。
fn encode_image(
    image: &image::DynamicImage,
    preferred_format: image::ImageFormat,
    icc_profile: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
) -> Result<(Vec<u8>, image::ImageFormat), String> {
    use image::ImageEncoder;
    let target_format = match preferred_format {
        image::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        image::ImageFormat::WebP => image::ImageFormat::WebP,
        _ => image::ImageFormat::Png,
    };
    let encode_err =
        |e: image::ImageError| format!("图片编码失败（{target_format:?}）：{e}");
    let apply_metadata = |encoder: &mut dyn ImageEncoder| -> Result<(), String> {
        if let Some(profile) = &icc_profile {
            encoder
                .set_icc_profile(profile.clone())
                .map_err(|e| format!("写入 ICC 元数据失败：{e}"))?;
        }
        if let Some(exif) = &exif {
            encoder
                .set_exif_metadata(exif.clone())
                .map_err(|e| format!("写入 EXIF 元数据失败：{e}"))?;
        }
        Ok(())
    };
    let mut buffer = Vec::new();
    match target_format {
        image::ImageFormat::Png => {
            let rgba = image.to_rgba8();
            let mut encoder = image::codecs::png::PngEncoder::new(&mut buffer);
            apply_metadata(&mut encoder)?;
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    image::ColorType::Rgba8.into(),
                )
                .map_err(encode_err)?;
        }
        image::ImageFormat::Jpeg => {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, 85);
            apply_metadata(&mut encoder)?;
            encoder.encode_image(image).map_err(encode_err)?;
        }
        image::ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut buffer);
            apply_metadata(&mut encoder)?;
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    image::ColorType::Rgba8.into(),
                )
                .map_err(encode_err)?;
        }
        _ => unreachable!("不支持的目标格式已在前置匹配收窄"),
    }
    Ok((buffer, target_format))
}

fn format_to_mime(format: image::ImageFormat) -> String {
    match format {
        image::ImageFormat::Jpeg => "image/jpeg".to_string(),
        image::ImageFormat::Gif => "image/gif".to_string(),
        image::ImageFormat::WebP => "image/webp".to_string(),
        _ => "image/png".to_string(),
    }
}

/// 有界读取：最多 `MAX_INPUT_FILE_BYTES + 1` 字节，超限即报错（不整读大文件）。
fn read_bounded(file: &mut std::fs::File, path: &Path) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let limit = (MAX_INPUT_FILE_BYTES + 1) as u64;
    let mut handle = file.take(limit);
    handle
        .read_to_end(&mut buf)
        .map_err(|e| format!("图片文件读取失败（{}）：{e}", path.display()))?;
    if buf.len() > MAX_INPUT_FILE_BYTES {
        return Err(format!(
            "输入文件 {} 字节超过 {} MiB 上限（{}）",
            buf.len(),
            MAX_INPUT_FILE_BYTES / 1024 / 1024,
            path.display()
        ));
    }
    Ok(buf)
}

/// 打开并复核为普通文件：Linux 以 O_NONBLOCK 打开避免 FIFO 阻塞，再按句柄
/// 元数据复核文件类型；目录/设备/FIFO/缺失/不可读均明确报错（§3.1）。
fn open_regular_file(path: &Path) -> Result<std::fs::File, String> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| format!("图片路径无法打开（{}）：{e}", path.display()))?
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path)
        .map_err(|e| format!("图片路径无法打开（{}）：{e}", path.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("图片文件元数据读取失败（{}）：{e}", path.display()))?;
    if !meta.file_type().is_file() {
        let kind = if meta.file_type().is_dir() {
            "目录"
        } else {
            "设备/FIFO/套接字等特殊文件"
        };
        return Err(format!("图片路径不是普通文件（{}）：{kind}", path.display()));
    }
    Ok(file)
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use image::ImageDecoder;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "tchat-vimg-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn png_bytes(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Rgba(rgba));
        let mut cursor = Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, ImageFormat::Png)
            .expect("encode test image");
        cursor.into_inner()
    }

    fn noise_png(width: u32, height: u32, seed: u64) -> Vec<u8> {
        let mut img = ImageBuffer::new(width, height);
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        for (_, _, px) in img.enumerate_pixels_mut() {
            *px = Rgba([next(), next(), next(), 255]);
        }
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        cursor.into_inner()
    }

    fn gif_bytes() -> Vec<u8> {
        // 1×1 红色 GIF87a（固定字节，无需 gif 编码器）
        vec![
            0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
            0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
        ]
    }

    /// 上游 image_tests：小图（无需缩放）PNG/JPEG/WebP 源字节逐字节保留。
    #[test]
    fn small_png_bytes_preserved() {
        let bytes = png_bytes(3, 2, [10, 20, 30, 255]);
        let enc = prepare_image(Path::new("a.png"), bytes.clone()).unwrap();
        assert_eq!(enc.bytes, bytes, "小 PNG 源字节保留");
        assert_eq!(enc.mime, "image/png");
        assert_eq!((enc.width, enc.height), (3, 2));
        assert_eq!((enc.source_width, enc.source_height), (3, 2));
    }

    /// 上游样例（§7）：2304×864 → HIGH_DETAIL 缩为 2048×768。
    #[test]
    fn high_detail_resizes_2304x864_to_2048x768() {
        let bytes = png_bytes(2304, 864, [1, 2, 3, 255]);
        let enc = prepare_image(Path::new("wide.png"), bytes).unwrap();
        assert_eq!((enc.width, enc.height), (2048, 768));
        assert_eq!((enc.source_width, enc.source_height), (2304, 864));
        // 解码后的请求图片确实可重新打开且尺寸一致
        let reopened = image::load_from_memory(&enc.bytes).unwrap();
        assert_eq!(reopened.dimensions(), (2048, 768));
    }

    /// 长图（竖向）按同一策略缩放。
    #[test]
    fn tall_image_resized() {
        let enc =
            prepare_image(Path::new("tall.png"), png_bytes(864, 2304, [5, 6, 7, 255])).unwrap();
        assert_eq!((enc.width, enc.height), (768, 2048));
    }

    /// patch 预算：2048×2048 经 max-dimension 不缩，但 64×64=4096 patches 超过
    /// 2500，按面积缩到 1600×1600（50×50=2500）。
    #[test]
    fn patch_budget_limits_square_image() {
        assert_eq!(high_detail_output_dimensions(2048, 2048), (1600, 1600));
        assert_eq!(high_detail_output_dimensions(1600, 1600), (1600, 1600));
        assert_eq!(high_detail_output_dimensions(2304, 864), (2048, 768));
        assert_eq!(high_detail_output_dimensions(100, 50), (100, 50), "不放大");
        assert!(dimensions_fit(64, 32));
        assert!(!dimensions_fit(2048, 2048));
    }

    /// GIF 解码后转静态 PNG（上游行为）。
    #[test]
    fn gif_converted_to_static_png() {
        let enc = prepare_image(Path::new("a.gif"), gif_bytes()).unwrap();
        assert_eq!(enc.mime, "image/png");
        let reopened = image::load_from_memory(&enc.bytes).unwrap();
        assert_eq!(reopened.dimensions(), (1, 1));
    }

    /// 上游行为：小图 JPEG/WebP 源字节保留（无需缩放时透传）。
    #[test]
    fn small_jpeg_and_webp_bytes_preserved() {
        // JPEG 不支持 RGBA：用 RGB8 夹具
        let img = ImageBuffer::from_pixel(4, 4, image::Rgb([90u8, 120, 150]));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Jpeg).unwrap();
        let jpeg = cursor.into_inner();
        let enc = prepare_image(Path::new("a.jpg"), jpeg.clone()).unwrap();
        assert_eq!(enc.bytes, jpeg, "小 JPEG 源字节保留");
        assert_eq!(enc.mime, "image/jpeg");

        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::WebP).unwrap();
        let webp = cursor.into_inner();
        let enc = prepare_image(Path::new("a.webp"), webp.clone()).unwrap();
        assert_eq!(enc.bytes, webp, "小 WebP 源字节保留");
        assert_eq!(enc.mime, "image/webp");
    }

    /// EXIF 元数据：重编码路径尽力保留（上游 apply_image_metadata 语义）。
    #[test]
    fn exif_metadata_preserved_when_reencoded() {
        use image::ImageEncoder;
        let exif = b"Exif\x00\x00MM\x00*\x00\x00\x00\x08\x00\x00".to_vec();
        let img = ImageBuffer::from_pixel(2304, 864, Rgba([9, 9, 9, 255]));
        let mut cursor = Cursor::new(Vec::new());
        let mut encoder = image::codecs::png::PngEncoder::new(&mut cursor);
        encoder.set_exif_metadata(exif.clone()).unwrap();
        encoder
            .write_image(img.as_raw(), 2304, 864, image::ColorType::Rgba8.into())
            .unwrap();
        let enc = prepare_image(Path::new("exif.png"), cursor.into_inner()).unwrap();
        assert_eq!(enc.mime, "image/png");
        assert_eq!((enc.width, enc.height), (2048, 768), "触发了重编码");
        let mut decoder = image::ImageReader::new(Cursor::new(&enc.bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.exif_metadata().unwrap(), Some(exif), "EXIF 保留");
    }

    /// 设备路径（/dev/null＝字符设备）明确拒绝，不当图片处理（Unix）。
    #[test]
    fn device_path_rejected() {
        #[cfg(unix)]
        {
            let outcome = execute(
                r#"{ "path": "/dev/null" }"#,
                Path::new("/"),
                None,
            );
            let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
            assert_eq!(v["ok"], serde_json::json!(false));
            assert!(
                v["error"].as_str().unwrap().contains("普通文件"),
                "设备错误：{}",
                v["error"]
            );
            assert!(outcome.image.is_none());
        }
    }

    /// 非图片内容：明确失败，不产出 data URL。
    #[test]
    fn non_image_rejected() {
        let err = prepare_image(Path::new("a.txt"), b"not an image".to_vec()).unwrap_err();
        assert!(err.contains("无法识别"), "实际错误：{err}");
    }

    /// SVG/PDF 不在首版解码范围：按内容识别拒绝。
    #[test]
    fn svg_rejected_by_content() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let err = prepare_image(Path::new("a.svg"), svg.to_vec()).unwrap_err();
        assert!(err.contains("无法识别") || err.contains("解码"), "实际错误：{err}");
    }

    /// 解码前宽高上限：IHDR 声明 20000×100 的 PNG 在限额处失败。
    #[test]
    fn decode_dimension_limit_enforced() {
        // 手工构造 PNG：签名 + IHDR(20000×100, 8bit RGBA) + IEND（无 IDAT）
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&100u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        let err = prepare_image(Path::new("huge.png"), png).unwrap_err();
        assert!(err.contains("16384") || err.contains("上限"), "实际错误：{err}");
    }

    /// 准备后 4 MiB 上限：2048×2048 噪声图缩到 1600×1600 后仍远超 4 MiB → 失败。
    #[test]
    fn prepared_size_limit_enforced() {
        let bytes = noise_png(2048, 2048, 42);
        let err = prepare_image(Path::new("noise.png"), bytes).unwrap_err();
        assert!(err.contains("4 MiB"), "实际错误：{err}");
    }

    /// RGB ICC profile：重编码路径尽力保留（上游 bytes 16..20 == b"RGB " 过滤）。
    #[test]
    fn rgb_icc_profile_preserved_when_reencoded() {
        let mut profile = vec![0u8; 128];
        profile[16..20].copy_from_slice(b"RGB ");
        let bytes = png_with_icc(2304, 864, &profile);
        let enc = prepare_image(Path::new("icc.png"), bytes).unwrap();
        assert_eq!(enc.mime, "image/png");
        // 触发了缩放重编码；输出仍带同一 ICC
        let mut decoder = image::ImageReader::new(Cursor::new(&enc.bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        let out_profile = decoder.icc_profile().unwrap().expect("输出应保留 RGB ICC");
        assert_eq!(out_profile, profile);
    }

    fn png_with_icc(width: u32, height: u32, profile: &[u8]) -> Vec<u8> {
        let img = ImageBuffer::from_pixel(width, height, Rgba([9, 9, 9, 255]));
        let mut cursor = Cursor::new(Vec::new());
        let mut encoder = image::codecs::png::PngEncoder::new(&mut cursor);
        use image::ImageEncoder;
        encoder.set_icc_profile(profile.to_vec()).unwrap();
        encoder
            .write_image(
                img.as_raw(),
                width,
                height,
                image::ColorType::Rgba8.into(),
            )
            .unwrap();
        cursor.into_inner()
    }

    // ---------- execute 契约 ----------

    fn write_png(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, png_bytes(2, 3, [1, 2, 3, 255])).unwrap();
        p
    }

    #[test]
    fn execute_success_summary_and_payload() {
        let t = Tmp::new("ok");
        let p = write_png(&t.0, "img.png");
        let outcome = execute(
            &serde_json::json!({ "path": p.display().to_string() }).to_string(),
            &t.0,
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "{}", outcome.summary);
        assert_eq!(v["path"], serde_json::json!(p.display().to_string()));
        assert_eq!(v["width"], serde_json::json!(2));
        assert_eq!(v["height"], serde_json::json!(3));
        let image = outcome.image.expect("成功必有载荷");
        assert_eq!(image.detail, "high");
        assert!(image.image_url.starts_with("data:image/png;base64,"));
        // data URL 可解码回原字节（小图保留源字节）
        let b64 = image.image_url.strip_prefix("data:image/png;base64,").unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
            std::fs::read(&p).unwrap()
        );
    }

    #[test]
    fn execute_relative_path_resolved_against_cwd() {
        let t = Tmp::new("rel");
        write_png(&t.0, "中文 图.png");
        let outcome = execute(
            r#"{ "path": "中文 图.png" }"#,
            &t.0,
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "{}", outcome.summary);
        assert!(v["path"].as_str().unwrap().contains("中文 图.png"));
        assert!(outcome.image.is_some());
    }

    #[test]
    fn execute_symlink_to_regular_file_accepted() {
        let t = Tmp::new("link");
        let p = write_png(&t.0, "real.png");
        let link = t.0.join("alias.png");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        let outcome = execute(
            &serde_json::json!({ "path": "alias.png" }).to_string(),
            &t.0,
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "{}", outcome.summary);
    }

    #[test]
    fn execute_directory_device_fifo_missing_rejected() {
        let t = Tmp::new("bad");
        let dir_arg = |rel: &str| {
            execute(
                &serde_json::json!({ "path": rel }).to_string(),
                &t.0,
                None,
            )
        };
        // 目录
        let v: serde_json::Value = serde_json::from_str(&dir_arg(".").summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false));
        assert!(
            v["error"].as_str().unwrap().contains("普通文件"),
            "目录错误：{}",
            v["error"]
        );
        // FIFO：不得阻塞，直接失败
        let fifo = t.0.join("pipe");
        #[cfg(unix)]
        {
            let nix = std::process::Command::new("mkfifo").arg(&fifo).status().unwrap();
            assert!(nix.success());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let fifo_arg = serde_json::json!({ "path": fifo.display().to_string() }).to_string();
        let cwd = t.0.clone();
        std::thread::spawn(move || {
            let o = execute(&fifo_arg, &cwd, None);
            tx.send(o.summary).unwrap();
        });
        let summary = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("FIFO 不得阻塞");
        let v: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "{summary}");
        // 缺失
        let v: serde_json::Value = serde_json::from_str(&dir_arg("nope.png").summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false));
        let e = v["error"].as_str().unwrap();
        assert!(e.contains("打开") || e.contains("读取"), "{e}");
    }

    #[test]
    fn execute_invalid_arguments_rejected() {
        let t = Tmp::new("args");
        for bad in [
            "not json",
            "{}",
            r#"{ "path": 5 }"#,
            r#"{ "path": "" }"#,
            r#"{ "path": null }"#,
        ] {
            let outcome = execute(bad, &t.0, None);
            let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
            assert_eq!(v["ok"], serde_json::json!(false), "参数 {bad}");
            assert!(outcome.image.is_none(), "失败不携带载荷");
        }
    }

    #[test]
    fn execute_cancel_checkpoints() {
        let t = Tmp::new("cancel");
        write_png(&t.0, "img.png");
        let cancel = AtomicBool::new(true);
        // 读取前取消：即使路径是缺失文件也先报取消
        let outcome = execute(
            r#"{ "path": "missing.png" }"#,
            &t.0,
            Some(&cancel),
        );
        let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false));
        assert!(v["error"].as_str().unwrap().contains("取消"), "{}", v["error"]);
        assert!(outcome.image.is_none());
    }

    #[test]
    fn execute_input_size_limit() {
        let t = Tmp::new("big");
        let p = t.0.join("big.png");
        let mut f = std::fs::File::create(&p).unwrap();
        // 精确超限 1 字节；读取在解码前有界截断
        f.write_all(&vec![0u8; MAX_INPUT_FILE_BYTES + 1]).unwrap();
        drop(f);
        let outcome = execute(
            &serde_json::json!({ "path": "big.png" }).to_string(),
            &t.0,
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&outcome.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "{}", outcome.summary);
        assert!(v["error"].as_str().unwrap().contains("20 MiB"), "{}", v["error"]);
        assert!(outcome.image.is_none());
    }

    #[test]
    fn unsupported_outcome_does_not_read_file() {
        let t = Tmp::new("off");
        // 缺失路径也只报能力错误——不读取文件
        let o = unsupported_outcome();
        let v: serde_json::Value = serde_json::from_str(&o.summary).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false));
        assert!(v["error"].as_str().unwrap().contains("未确认支持图片输入"));
        assert!(o.image.is_none());
        let _ = t;
    }

    #[test]
    fn tool_definition_shape() {
        let def = tool_definition();
        assert_eq!(def["type"], serde_json::json!("function"));
        assert_eq!(def["function"]["name"], serde_json::json!("view_image"));
        assert_eq!(
            def["function"]["parameters"]["required"],
            serde_json::json!(["path"])
        );
        assert_eq!(
            def["function"]["parameters"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert!(def["function"]["parameters"]["properties"]["path"]["type"] == "string");
    }
}
