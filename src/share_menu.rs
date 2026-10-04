//! 兼容既有调用方的右键菜单操作入口；所有前端的操作均由 ops 编排。

pub use crate::ops::{apply_share_menu as apply, revert_share_menu as revert};
