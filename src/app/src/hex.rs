//! S19：sha2 0.11 不带 hex 输出——全仓唯一下游编码点，调用方自定截断
//! （catalog/naming 取前 16 字符，session/runner 取前 16 字节）。

/// 字节序列的小写十六进制编码。
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
