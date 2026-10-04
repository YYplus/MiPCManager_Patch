//! Windows 11 小米互传右键菜单的高层操作封装。

use crate::patches::xiaomi_share_menu;
use anyhow::Result;

pub fn apply() -> Result<Vec<String>> {
    let outcome = xiaomi_share_menu::apply()?;
    Ok(match outcome {
        xiaomi_share_menu::PatchOutcome::Applied => {
            vec!["✓ 已启用 Windows 11 一级右键“使用小米互传发送”".to_string()]
        }
        xiaomi_share_menu::PatchOutcome::AlreadyApplied => {
            vec!["• Windows 11 右键小米互传已启用（跳过）".to_string()]
        }
        _ => Vec::new(),
    })
}

pub fn revert() -> Result<Vec<String>> {
    let outcome = xiaomi_share_menu::revert()?;
    Ok(match outcome {
        xiaomi_share_menu::PatchOutcome::Reverted => {
            vec!["✓ 已关闭 Windows 11 一级右键小米互传并清理相关组件".to_string()]
        }
        xiaomi_share_menu::PatchOutcome::AlreadyReverted => {
            vec!["• Windows 11 右键小米互传已关闭（跳过）".to_string()]
        }
        _ => Vec::new(),
    })
}
