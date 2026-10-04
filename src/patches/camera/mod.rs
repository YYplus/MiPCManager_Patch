pub mod dotnet;

use self::dotnet::{InjectOutcome, inject_guard_return};
use crate::{infra::pe::PeImage, install};
use anyhow::Result;
use std::path::Path;

/// 目标程序集文件名。
pub const TARGET_DLL: &str = "PcControlCenter.dll";

/// 目标类型简单名。
const TYPE_NAME: &str = "SynergyUIService";
/// 目标方法名后缀（显式接口实现，元数据中带接口全名前缀）。
const METHOD_SUFFIX: &str = "ExceptionCallback";
/// 实例方法参数下标：arg0=this，arg1=exception_id。
const ARG_INDEX: u8 = 1;
/// `CameraExceptionId.kLOCAL_CAMERA_DISABLED` 的枚举值。
const KLOCAL_CAMERA_DISABLED: i32 = 3;
/// 注入方法体所在新节名（≤8 字节）。
const SECTION_NAME: &str = ".mipatch";

/// 对 `PcControlCenter.dll` 应用补丁。
pub fn apply(dll_path: &Path) -> Result<InjectOutcome> {
    install::ensure_backup(dll_path)?;
    let data = std::fs::read(dll_path)?;
    let mut pe = PeImage::parse(data)?;
    let outcome = inject_guard_return(
        &mut pe,
        TYPE_NAME,
        METHOD_SUFFIX,
        ARG_INDEX,
        KLOCAL_CAMERA_DISABLED,
        SECTION_NAME,
    )?;
    if outcome == InjectOutcome::Patched {
        install::write_file_atomic(dll_path, &pe.data)?;
    }
    Ok(outcome)
}

/// 从备份还原 `PcControlCenter.dll`。
pub fn revert(dll_path: &Path) -> Result<()> {
    install::restore_backup(dll_path)
}

/// Check the actual injected IL guard, independent of unrelated file changes.
pub fn is_patched(dll_path: &Path) -> bool {
    let Ok(data) = std::fs::read(dll_path) else {
        return false;
    };
    let Ok(pe) = PeImage::parse(data) else {
        return false;
    };
    let Ok(method) = dotnet::metadata::find_method(&pe, TYPE_NAME, METHOD_SUFFIX) else {
        return false;
    };
    let Some(offset) = pe.rva_to_offset(method.body_rva) else {
        return false;
    };
    let Ok(body) = dotnet::method_body::MethodBody::parse(&pe.data, offset) else {
        return false;
    };
    body.il.starts_with(&dotnet::method_body::build_guard(
        ARG_INDEX,
        KLOCAL_CAMERA_DISABLED,
    ))
}
