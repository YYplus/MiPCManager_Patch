//! SMBIOS 设备身份补丁（call 指令重定向）。
//!
//! `micont_rtm.dll` 通过 `GetSystemFirmwareTable` 读取主板 SMBIOS，
//! 将真实 `project_id` 通过 Lyra 上报给手机，导致妙播无法发现非小米设备。
//!
//! 方案：找到 `call [rip+disp32]` 指令（指向 GetSystemFirmwareTable 的 IAT），
//! 原地改写为 `E8 rel32 + NOP`（6 字节不变），直接跳转到 `.mipatch` 节中的
//! trampoline；trampoline 通过原始 IAT 调用原函数并替换 SMBIOS buffer 字段。
//!
//! 与 LocaleSpoof 均修改 `micont_rtm.dll`。二者共享原始备份，但还原时会保留
//! 另一项补丁的独立状态。

use super::{smbios, x64_trampoline};
use crate::infra::pe::PeImage;
use crate::install;
use crate::patches::locale;
use anyhow::{Context, Result, bail};
use std::path::Path;

pub const TARGET_DLL: &str = "micont_rtm.dll";
pub const DEFAULT_MODEL: &str = "TM2425";

const SECTION_NAME: &str = ".mipatch";
const SECTION_CHARACTERISTICS: u32 = 0x6000_0020;

#[derive(Debug, PartialEq, Eq)]
pub enum PatchOutcome {
    Patched,
    AlreadyPatched,
}

pub fn apply(dll_path: &Path, model: Option<&str>) -> Result<PatchOutcome> {
    if is_patched(dll_path) {
        return Ok(PatchOutcome::AlreadyPatched);
    }

    let model_code = model.unwrap_or(DEFAULT_MODEL);

    let smbios_raw = read_system_smbios()?;
    let table = smbios::SmbiosTable::new(smbios_raw);
    let fields = table.extract_fields()?;

    let entries: Vec<_> = fields
        .iter()
        .map(|f| build_replace_entry(f, model_code))
        .collect();
    if entries.is_empty() {
        bail!("未找到可替换的 SMBIOS 字段");
    }

    apply_with_entries(dll_path, &entries)
}

fn apply_with_entries(
    dll_path: &Path,
    entries: &[x64_trampoline::ReplaceEntry],
) -> Result<PatchOutcome> {
    if is_patched(dll_path) {
        return Ok(PatchOutcome::AlreadyPatched);
    }

    install::ensure_backup(dll_path)?;
    let data = std::fs::read(dll_path)?;
    let mut pe = PeImage::parse(data)?;

    let (_, iat_rva, _) = pe
        .find_iat_entry("kernel32", "GetSystemFirmwareTable")
        .context("未找到 kernel32!GetSystemFirmwareTable")?;

    let (call_off, call_rva) = pe
        .find_call_to_iat(iat_rva)
        .context("未找到指向 GetSystemFirmwareTable IAT 的 call 指令")?;

    let tc = x64_trampoline::build_trampoline(iat_rva, entries);
    let trampoline_rva = pe.append_section(SECTION_NAME, &tc.bytes, SECTION_CHARACTERISTICS)?;

    let sections = pe.sections();
    let sec = sections.last().context("append_section 未产生节")?;
    let raw_start = sec.raw_pointer as usize;
    let next_rip = trampoline_rva.wrapping_add(tc.iat_disp_byte_offset as u32 + 4);
    let correct_disp = (iat_rva as i64).wrapping_sub(next_rip as i64) as i32;
    pe.data[raw_start + tc.iat_disp_byte_offset..raw_start + tc.iat_disp_byte_offset + 4]
        .copy_from_slice(&correct_disp.to_le_bytes());

    let call_disp = (trampoline_rva as i64).wrapping_sub(call_rva.wrapping_add(5) as i64) as i32;
    pe.data[call_off] = 0xE8;
    pe.data[call_off + 1..call_off + 5].copy_from_slice(&call_disp.to_le_bytes());
    pe.data[call_off + 5] = 0x90;

    pe.update_checksum();
    install::write_file_atomic(dll_path, &pe.data)?;

    Ok(PatchOutcome::Patched)
}

/// 还原 SMBIOS 补丁，同时保留同一 DLL 中已经启用的地区伪装。
pub fn revert(dll_path: &Path) -> Result<()> {
    if !is_patched(dll_path) {
        return Ok(());
    }
    let locale_was_patched = locale::is_patched(dll_path);
    let mut data = std::fs::read(install::backup_path(dll_path))?;
    // The first backup can itself contain locale spoofing (SMBIOS applied
    // afterwards). Preserve the current on AND off states before one write.
    if locale_was_patched {
        locale::patch_bytes(&mut data)?;
    } else {
        locale::revert_bytes(&mut data)?;
    }
    install::write_file_atomic(dll_path, &data)?;
    Ok(())
}

/// 判断当前文件是否真正含 SMBIOS call 重定向，而不是仅凭共享 `.orig.bak` 推断。
///
/// 校验原 call 的跳转目标、新节中的 trampoline 和对同一 firmware IAT 的引用。
/// 地区备份或无关的 E8 指令不能作为 SMBIOS 已应用的依据。
pub fn is_patched(dll_path: &Path) -> bool {
    let backup = install::backup_path(dll_path);
    let (Ok(current), Ok(original)) = (std::fs::read(dll_path), std::fs::read(backup)) else {
        return false;
    };
    let Ok(original_pe) = PeImage::parse(original) else {
        return false;
    };
    let Ok((_, iat_rva, _)) = original_pe.find_iat_entry("kernel32", "GetSystemFirmwareTable")
    else {
        return false;
    };
    let Ok((_, call_rva)) = original_pe.find_call_to_iat(iat_rva) else {
        return false;
    };
    let Ok(current_pe) = PeImage::parse(current) else {
        return false;
    };
    let Some(call_off) = current_pe.rva_to_offset(call_rva) else {
        return false;
    };
    let Some(call) = current_pe.data.get(call_off..call_off + 6) else {
        return false;
    };
    if call[0] != 0xE8 || call[5] != 0x90 {
        return false;
    }
    let disp = i32::from_le_bytes(call[1..5].try_into().unwrap());
    let target = call_rva.wrapping_add(5).wrapping_add_signed(disp);
    let original_sections = original_pe.sections();
    if !current_pe.sections().iter().any(|section| {
        section.virtual_address == target
            && !original_sections
                .iter()
                .any(|old| old.virtual_address == target)
    }) {
        return false;
    }
    let Some(start) = current_pe.rva_to_offset(target) else {
        return false;
    };
    let Ok((_, current_iat, _)) = current_pe.find_iat_entry("kernel32", "GetSystemFirmwareTable")
    else {
        return false;
    };
    if current_iat != iat_rva {
        return false;
    }

    let signature = x64_trampoline::build_trampoline(
        iat_rva,
        &[x64_trampoline::ReplaceEntry {
            offset: 0,
            verify: Vec::new(),
            replace: Vec::new(),
        }],
    );
    let offset = signature.iat_disp_byte_offset;
    let prefix = &signature.bytes[..offset];
    // Recognize the previous generator too, so an existing patch can be
    // reverted before installing the corrected trampoline.
    let legacy_prefix: &[u8] = &[
        0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x48, 0x83, 0xEC, 0x28, 0x48, 0x89, 0xCB, 0x48,
        0x89, 0xD6, 0x4C, 0x89, 0xC7, 0x48, 0x8D, 0x05,
    ];
    let after_lea = &signature.bytes[offset + 4..offset + 22];
    [(prefix, 4u32), (legacy_prefix, 7u32)]
        .iter()
        .any(|&(prefix, next)| {
            let displacement = start + prefix.len();
            let Some(bytes) = current_pe.data.get(start..displacement + 22) else {
                return false;
            };
            let encoded =
                i32::from_le_bytes(bytes[prefix.len()..prefix.len() + 4].try_into().unwrap());
            bytes.starts_with(prefix)
                && &bytes[prefix.len() + 4..] == after_lea
                && target
                    .wrapping_add(prefix.len() as u32 + next)
                    .wrapping_add_signed(encoded)
                    == iat_rva
        })
}

// ── helpers ────────────────────────────────────────────────

fn build_replace_entry(
    field: &smbios::SmbiosField,
    model_code: &str,
) -> x64_trampoline::ReplaceEntry {
    let target = field.role.target_value(model_code);
    let mut verify = field.value.as_bytes().to_vec();
    verify.push(0);
    let mut replace = target.as_bytes().to_vec();
    if replace.len() < verify.len() {
        replace.resize(verify.len(), 0);
    }
    x64_trampoline::ReplaceEntry {
        offset: field.offset as u32,
        verify,
        replace,
    }
}

#[cfg(windows)]
fn read_system_smbios() -> Result<Vec<u8>> {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemFirmwareTable(
            provider: u32,
            table_id: u32,
            buffer: *mut c_void,
            buffer_size: u32,
        ) -> u32;
    }

    const RSMB: u32 = 0x52534D42;

    // SAFETY: FFI call to kernel32; parameters are trivially safe.
    unsafe {
        let size = GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0);
        if size == 0 {
            bail!("GetSystemFirmwareTable('RSMB') 返回 0");
        }
        let mut buf = vec![0u8; size as usize];
        let written = GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr().cast(), size);
        if written == 0 || written > size {
            bail!("GetSystemFirmwareTable 读取失败: written={written}, expected={size}");
        }
        buf.truncate(written as usize);
        Ok(buf)
    }
}

#[cfg(not(windows))]
fn read_system_smbios() -> Result<Vec<u8>> {
    bail!("SMBIOS 补丁仅支持 Windows")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // A self-contained PE64 with a real import table, indirect firmware call
    // and locale string. No installed Xiaomi software or system changes.
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TARGET_DLL);
        let mut bytes = vec![0; 0x800];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&0xF0u16.to_le_bytes());
        bytes[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
        for (offset, value) in [
            (0x98 + 32, 0x1000u32),
            (0x98 + 36, 0x200),
            (0x98 + 56, 0x2000),
            (0x98 + 60, 0x200),
            (0x98 + 108, 16),
            (0x98 + 120, 0x1100),
            (0x98 + 124, 40),
            (0x188 + 8, 0x600),
            (0x188 + 12, 0x1000),
            (0x188 + 16, 0x600),
            (0x188 + 20, 0x200),
            (0x188 + 36, SECTION_CHARACTERISTICS),
            (0x300, 0x1180),
            (0x30C, 0x1160),
            (0x310, 0x1200),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x188..0x18D].copy_from_slice(b".text");
        bytes[0x360..0x36D].copy_from_slice(b"kernel32.dll\0");
        bytes[0x380..0x388].copy_from_slice(&0x11C0u64.to_le_bytes());
        bytes[0x400..0x408].copy_from_slice(&0x11C0u64.to_le_bytes());
        let name = b"GetSystemFirmwareTable\0";
        bytes[0x3C2..0x3C2 + name.len()].copy_from_slice(name);
        bytes[0x220..0x222].copy_from_slice(&[0xFF, 0x15]);
        bytes[0x222..0x226].copy_from_slice(&(0x1200i32 - 0x1026).to_le_bytes());
        let geo: Vec<_> = "Geo\0Name\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        bytes[0x460..0x460 + geo.len()].copy_from_slice(&geo);
        fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn apply_smbios(path: &Path) -> PatchOutcome {
        apply_with_entries(
            path,
            &[x64_trampoline::ReplaceEntry {
                offset: 8,
                verify: b"Original model\0".to_vec(),
                replace: b"TM2425\0".to_vec(),
            }],
        )
        .unwrap()
    }

    #[test]
    fn locale_only_does_not_report_smbios() {
        let (_dir, path) = fixture();
        let original = fs::read(&path).unwrap();
        locale::apply(&path, "CN", false).unwrap();
        assert!(locale::is_patched(&path));
        assert!(!is_patched(&path));
        locale::revert(&path, false).unwrap();
        locale::revert(&path, false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn smbios_only_and_repeated_operations_are_idempotent() {
        let (_dir, path) = fixture();
        let original = fs::read(&path).unwrap();
        assert_eq!(apply_smbios(&path), PatchOutcome::Patched);
        assert!(is_patched(&path));
        assert!(!locale::is_patched(&path));
        let patched = fs::read(&path).unwrap();
        assert_eq!(apply_smbios(&path), PatchOutcome::AlreadyPatched);
        assert_eq!(fs::read(&path).unwrap(), patched);
        revert(&path).unwrap();
        revert(&path).unwrap();
        assert!(!is_patched(&path));
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn either_apply_order_preserves_the_other_patch_on_revert() {
        for locale_first in [false, true] {
            for revert_locale_first in [false, true] {
                let (_dir, path) = fixture();
                let original = fs::read(&path).unwrap();
                if locale_first {
                    locale::apply(&path, "CN", false).unwrap();
                }
                apply_smbios(&path);
                locale::apply(&path, "CN", false).unwrap();
                assert_eq!(
                    locale::apply(&path, "CN", false).unwrap(),
                    locale::PatchOutcome::AlreadyPatched
                );
                assert_eq!(apply_smbios(&path), PatchOutcome::AlreadyPatched);
                assert!(is_patched(&path) && locale::is_patched(&path));

                if revert_locale_first {
                    locale::revert(&path, false).unwrap();
                    locale::revert(&path, false).unwrap();
                    assert!(!locale::is_patched(&path));
                    assert!(is_patched(&path));
                    revert(&path).unwrap();
                } else {
                    revert(&path).unwrap();
                    revert(&path).unwrap();
                    assert!(!is_patched(&path));
                    assert!(locale::is_patched(&path));
                    locale::revert(&path, false).unwrap();
                }
                assert!(!is_patched(&path) && !locale::is_patched(&path));
                assert_eq!(fs::read(&path).unwrap(), original);
            }
        }
    }

    #[test]
    fn status_rejects_unrelated_call_and_invalid_iat_target() {
        let (_dir, path) = fixture();
        install::ensure_backup(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(&[0; 512]);
        bytes[0x220] = 0xE8;
        bytes[0x225] = 0x90;
        fs::write(&path, bytes).unwrap();
        assert!(!is_patched(&path));

        install::restore_backup(&path).unwrap();
        apply_smbios(&path);
        let mut pe = PeImage::parse(fs::read(&path).unwrap()).unwrap();
        let (_, iat, _) = pe
            .find_iat_entry("kernel32", "GetSystemFirmwareTable")
            .unwrap();
        let section = *pe.sections().last().unwrap();
        let tc = x64_trampoline::build_trampoline(
            iat,
            &[x64_trampoline::ReplaceEntry {
                offset: 0,
                verify: Vec::new(),
                replace: Vec::new(),
            }],
        );
        let displacement = section.raw_pointer as usize + tc.iat_disp_byte_offset;
        let actual =
            i32::from_le_bytes(pe.data[displacement..displacement + 4].try_into().unwrap());
        assert_eq!(
            section
                .virtual_address
                .wrapping_add(tc.iat_disp_byte_offset as u32 + 4)
                .wrapping_add_signed(actual),
            iat
        );
        pe.data[displacement] ^= 1;
        fs::write(&path, pe.data).unwrap();
        assert!(!is_patched(&path));
    }

    #[test]
    fn prepatched_backup_does_not_resurrect_reverted_locale() {
        let (_dir, path) = fixture();
        let original = fs::read(&path).unwrap();
        let mut prepatched = original.clone();
        locale::patch_bytes(&mut prepatched).unwrap();
        fs::write(&path, prepatched).unwrap();
        // SMBIOS now creates its first backup from an already spoofed DLL.
        apply_smbios(&path);
        locale::revert(&path, false).unwrap();
        assert!(!locale::is_patched(&path) && is_patched(&path));
        revert(&path).unwrap();
        revert(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!locale::is_patched(&path));
    }
}
