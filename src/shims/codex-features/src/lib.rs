//! codex-features shim（008 §3-J5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {} // 桩：变体按需补全
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {} // 桩
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {} // 桩
impl Features {
    pub fn is_enabled(&self, _f: Feature) -> bool { false }
}
pub const FEATURES: Features = Features {};
