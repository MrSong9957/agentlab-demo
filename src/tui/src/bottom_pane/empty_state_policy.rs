//! Only the ordinary composer opts into fresh-thread decoration; other views stay undecorated.

use super::*;
use crate::empty_state_animation::ComposerState;

impl BottomPane {
    pub(crate) fn empty_state_composer(&self) -> Option<ComposerState> {
        // 008-TRIM: warnings/questions/inline_banner 条件（裁剪功能，008 §1）
        match self.view_stack.as_slice() {
            [] => self.composer.empty_state_composer(),
            _ => None,
        }
    }
}
