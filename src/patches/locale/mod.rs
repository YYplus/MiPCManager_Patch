use crate::{infra, install};
use anyhow::{Result, bail};
use std::path::Path;

/// 目标 DLL 文件名。
pub const TARGET_DLL: &str = "micont_rtm.dll";

/// 锚点：宽字符串 `Geo\0`（紧邻待改值名之前，确保唯一）。
const ANCHOR_GEO: &[u8] = &[0x47, 0x00, 0x65, 0x00, 0x6F, 0x00, 0x00, 0x00];
/// 原值名：宽字符串 `Name` + 终止符（10 字节）。
const ORIG_NAME: &[u8] = &[0x4E, 0x00, 0x61, 0x00, 0x6D, 0x00, 0x65, 0x00, 0x00, 0x00];
/// 新值名：宽字符串 `XCN` + 终止符 + 补位（10 字节，与原值名等长）。
const PATCHED_NAME: &[u8] = &[0x58, 0x00, 0x43, 0x00, 0x4E, 0x00, 0x00, 0x00, 0x00, 0x00];

/// 注册表中用于伪装的值名与所在键。
const GEO_KEY: &str = r"Control Panel\International\Geo";
const SPOOF_VALUE_NAME: &str = "XCN";

#[derive(Debug, PartialEq, Eq)]
pub enum PatchOutcome {
    Patched,
    AlreadyPatched,
}

/// 在 DLL 字节中查找并应用值名替换。返回是否实际改动。
pub fn patch_bytes(data: &mut [u8]) -> Result<PatchOutcome> {
    let orig_sig = [ANCHOR_GEO, ORIG_NAME].concat();
    let patched_sig = [ANCHOR_GEO, PATCHED_NAME].concat();

    if let Some(pos) = infra::bytes::find_bytes(data, &orig_sig) {
        let name_at = pos + ANCHOR_GEO.len();
        data[name_at..name_at + PATCHED_NAME.len()].copy_from_slice(PATCHED_NAME);
        return Ok(PatchOutcome::Patched);
    }
    if infra::bytes::find_bytes(data, &patched_sig).is_some() {
        return Ok(PatchOutcome::AlreadyPatched);
    }
    bail!("未在 {TARGET_DLL} 中找到 `Geo\\Name` 特征，可能版本结构已变更");
}

/// 只撤销地区伪装对应的 `Geo\\XCN -> Geo\\Name` 字节，不整文件回滚。
///
/// `micont_rtm.dll` 还可能同时承载 Lyra SMBIOS 补丁，因此这里不能再直接恢复
/// `.orig.bak`，否则会把另一个独立补丁一并清掉。
pub(crate) fn revert_bytes(data: &mut [u8]) -> Result<bool> {
    let orig_sig = [ANCHOR_GEO, ORIG_NAME].concat();
    let patched_sig = [ANCHOR_GEO, PATCHED_NAME].concat();

    if let Some(pos) = infra::bytes::find_bytes(data, &patched_sig) {
        let name_at = pos + ANCHOR_GEO.len();
        data[name_at..name_at + ORIG_NAME.len()].copy_from_slice(ORIG_NAME);
        return Ok(true);
    }
    if infra::bytes::find_bytes(data, &orig_sig).is_some() {
        return Ok(false);
    }
    bail!("未在 {TARGET_DLL} 中找到 `Geo\\XCN` / `Geo\\Name` 特征，可能版本结构已变更");
}

/// 当前 DLL 是否真正处于地区伪装字节状态。
pub fn is_patched(dll_path: &Path) -> bool {
    let Ok(data) = std::fs::read(dll_path) else {
        return false;
    };
    let patched_sig = [ANCHOR_GEO, PATCHED_NAME].concat();
    infra::bytes::find_bytes(&data, &patched_sig).is_some()
}

/// 对安装目录中的 DLL 应用补丁，并写入注册表伪装值。
pub fn apply(dll_path: &Path, region: &str, write_registry: bool) -> Result<PatchOutcome> {
    install::ensure_backup(dll_path)?;
    let mut data = std::fs::read(dll_path)?;
    let outcome = patch_bytes(&mut data)?;
    if outcome == PatchOutcome::Patched {
        install::write_file_atomic(dll_path, &data)?;
    }
    if write_registry {
        set_registry(region)?;
    }
    Ok(outcome)
}

/// 仅还原地区伪装自身的字节并移除注册表伪装值；保留同一 DLL 中的其他补丁。
pub fn revert(dll_path: &Path, remove_registry: bool) -> Result<()> {
    let mut data = std::fs::read(dll_path)?;
    if revert_bytes(&mut data)? {
        install::write_file_atomic(dll_path, &data)?;
    }
    if remove_registry {
        remove_registry_value()?;
    }
    Ok(())
}

/// 写入 `HKCU\Control Panel\International\Geo\XCN = <region>`。
pub fn set_registry(region: &str) -> Result<()> {
    infra::registry::set_hkcu_string(GEO_KEY, SPOOF_VALUE_NAME, region)
}

fn remove_registry_value() -> Result<()> {
    infra::registry::delete_hkcu_value(GEO_KEY, SPOOF_VALUE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut buf = vec![0xAB; 4];
        buf.extend_from_slice(ANCHOR_GEO);
        buf.extend_from_slice(ORIG_NAME);
        buf.extend_from_slice(&[0xCD; 4]);
        buf
    }

    #[test]
    fn patch_then_idempotent() {
        let mut buf = fixture();
        let snapshot_len = buf.len();

        assert_eq!(patch_bytes(&mut buf).unwrap(), PatchOutcome::Patched);
        assert_eq!(buf.len(), snapshot_len, "补丁必须等长，不得移位");
        let name_at = 4 + ANCHOR_GEO.len();
        assert_eq!(&buf[name_at..name_at + PATCHED_NAME.len()], PATCHED_NAME);
        assert_eq!(patch_bytes(&mut buf).unwrap(), PatchOutcome::AlreadyPatched);
    }

    #[test]
    fn revert_only_changes_locale_signature() {
        let mut buf = fixture();
        let marker = b"other-patch-data";
        buf.extend_from_slice(marker);
        patch_bytes(&mut buf).unwrap();

        assert!(revert_bytes(&mut buf).unwrap());
        assert!(buf.ends_with(marker));
        assert!(!revert_bytes(&mut buf).unwrap());
    }

    /// 用未入库的厂商 DLL 验证真实 PcContinuity 版本。
    ///
    /// 运行时设置 `MIPCM_PCC_FIXTURE` 为 `micont_rtm.dll` 路径，并加 `--ignored`。
    #[test]
    #[ignore = "requires MIPCM_PCC_FIXTURE pointing to a real PcContinuity micont_rtm.dll"]
    fn patches_real_pc_continuity_fixture() {
        let path = std::env::var_os("MIPCM_PCC_FIXTURE")
            .map(std::path::PathBuf::from)
            .expect("MIPCM_PCC_FIXTURE must point to micont_rtm.dll");
        let original = std::fs::read(&path).expect("failed to read PcContinuity fixture");
        let mut patched = original.clone();

        assert_eq!(patch_bytes(&mut patched).unwrap(), PatchOutcome::Patched);
        assert_eq!(patched.len(), original.len());
        assert_eq!(
            original
                .iter()
                .zip(&patched)
                .filter(|(before, after)| before != after)
                .count(),
            4,
            "PcContinuity 样本应只改写 Name -> XCN 的 4 个非零字节"
        );
        assert_eq!(
            patch_bytes(&mut patched).unwrap(),
            PatchOutcome::AlreadyPatched
        );
        assert!(revert_bytes(&mut patched).unwrap());
        assert_eq!(patched, original);
    }
}
