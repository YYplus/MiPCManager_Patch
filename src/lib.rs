//! 小米电脑管家 / 小米互联 / 超级小爱补丁工具核心库。
//!
//! CLI 与 GUI 共享这里的核心逻辑，确保状态查看、各项补丁、安装卸载以及
//! Windows 11 小米互传右键菜单调用同一套实现。

pub mod i18n;
pub mod infra;
pub mod patches;

pub mod elevate;
pub mod ops;
pub mod share_menu;

pub mod experimental;
pub mod install;
pub mod uninstall;

#[cfg(feature = "cli")]
pub mod ui;
