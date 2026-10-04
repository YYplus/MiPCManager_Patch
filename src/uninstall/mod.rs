//! MiDrop Ext MSIX 包卸载、产品卸载（MiPCManager / PcContinuity）、服务清理、文件清理。
//!
//! 卸载为不可逆操作，调用方需在执行前获取用户确认。

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// MiDrop Ext MSIX 包名称。
const MSIX_PACKAGE_NAME: &str = "5f71dad9-3e77-4ada-9fad-12c2e761288f";

/// PcContinuity 相关服务。
const PC_CONTINUITY_SERVICES: &[&str] = &["micont_service", "MiPcContinuityService"];

/// MiPCManager 相关服务（包含 PcContinuity 共用项）。
const MIPC_MANAGER_SERVICES: &[&str] = &[
    "AIService",
    "MAFSvr",
    "MiDeviceService",
    "MiPlayCastService",
    "MiService",
    "micont_service",
    "dist_service",
    "DistributedService",
    "handoff_service",
];

use crate::infra::powershell::run_powershell;

// ── MSIX 包检测与卸载 ──

/// 检测 MiDrop Ext MSIX 包是否已安装。
/// 返回 `Some(PackageFullName)` 或 `None`。
pub fn detect_msix() -> Result<Option<String>> {
    let script = format!(
        "Get-AppxPackage -Name '{MSIX_PACKAGE_NAME}' | Select-Object -ExpandProperty PackageFullName"
    );
    let output = run_powershell(&script)?;
    let name = output.trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
}

/// 按 PackageFullName 卸载 MSIX 包。
pub fn remove_msix(package_full_name: &str) -> Result<()> {
    let script = format!("Remove-AppxPackage -Package '{package_full_name}'");
    run_powershell(&script)?;
    Ok(())
}

// ── 资源管理器重启 ──

/// 重启 Windows 资源管理器（explorer.exe）。
#[cfg(windows)]
pub fn restart_explorer() -> Result<()> {
    Command::new("taskkill")
        .args(["/f", "/im", "explorer.exe"])
        .status()
        .context("无法结束 explorer.exe")?;
    // 短暂等待确保进程退出
    std::thread::sleep(std::time::Duration::from_millis(500));
    Command::new("explorer.exe")
        .spawn()
        .context("无法启动 explorer.exe")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn restart_explorer() -> Result<()> {
    bail!("资源管理器重启仅支持 Windows")
}

// ── 服务管理 ──

/// 检查 Windows 服务是否存在。
pub fn service_exists(name: &str) -> Result<bool> {
    let script = format!(
        "Get-Service -Name '{name}' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name"
    );
    match run_powershell(&script) {
        Ok(output) => Ok(!output.trim().is_empty()),
        Err(_) => Ok(false),
    }
}

/// 删除 Windows 服务。返回 `Ok(true)` 表示已删除，`Ok(false)` 表示服务不存在。
pub fn remove_service(name: &str) -> Result<bool> {
    if !service_exists(name)? {
        return Ok(false);
    }
    let script = format!("Remove-Service -Name '{name}'");
    run_powershell(&script)?;
    Ok(true)
}

// ── 产品卸载 ──

/// 运行产品自带的 uninstall.exe，等待退出，验证是否成功删除自身。
/// 返回 `true` 表示 uninstall.exe 已被删除（卸载成功）。
///
/// Windows 卸载程序（尤其是 Inno Setup / NSIS 打包的程序）有时会先启动一个
/// GUI 进程，该进程随即启动真正的后台卸载子进程后退出。仅 `wait()` 主进程
/// 不足以确认卸载已完成。本函数在子进程退出后轮询检查 uninstall.exe 是否
/// 已被删除（最长等待 30 秒），确保返回时卸载程序确实已完成其清理工作。
pub fn run_product_uninstaller(uninstall_exe: &Path) -> Result<bool> {
    if !uninstall_exe.is_file() {
        bail!("未找到卸载程序：{}", uninstall_exe.display());
    }
    let status = Command::new(uninstall_exe)
        .spawn()
        .with_context(|| format!("无法启动卸载程序：{}", uninstall_exe.display()))?
        .wait()
        .context("等待卸载程序退出时出错")?;
    if !status.success() && uninstall_exe.is_file() {
        bail!("卸载程序未成功完成（退出码：{status}），已停止后续清理");
    }

    if !uninstall_exe.is_file() {
        return Ok(true);
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if !uninstall_exe.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 删除目录（若存在）。返回 `Ok(true)` 表示已删除，`Ok(false)` 表示不存在。
///
/// 在卸载流程中，子产品卸载程序可能异步释放文件句柄，因此在移除目录失败时
/// 会重试一次（等待 1 秒后），以规避文件锁冲突。
pub fn remove_dir_if_exists(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if std::fs::remove_dir_all(path).is_ok() {
        return Ok(true);
    }
    std::thread::sleep(Duration::from_secs(1));
    std::fs::remove_dir_all(path)
        .with_context(|| format!("无法删除目录（已重试）：{}", path.display()))?;
    Ok(true)
}

// ── 完整卸载流程 ──

/// 卸载小米电脑管家（完整流程）。
pub fn uninstall_xiaomi_pc_manager(root: &Path, log: &mut Vec<String>) -> Result<()> {
    log.push(format!("开始卸载小米电脑管家：{}", root.display()));

    // 1. 找到版本目录并运行 uninstall.exe
    let version = crate::install::latest_version_dir(root)?;
    let uninstall_exe = version.join("uninstall.exe");
    log.push(format!("  正在运行卸载程序：{}", uninstall_exe.display()));
    let removed = run_product_uninstaller(&uninstall_exe)?;
    if !removed {
        bail!(
            "主程序卸载未完成，已停止服务和文件清理：{}",
            uninstall_exe.display()
        );
    }
    log.push("  ✓ 主程序卸载完成".to_string());

    // 2. 卸载 AIService
    uninstall_sub_product(log, r"C:\Program Files\MI\AIService", "AIService");

    // 3. 卸载 MiService
    uninstall_sub_product(
        log,
        r"C:\Program Files (x86)\Timi Personal Computing\MiService",
        "MiService",
    );

    // 4. 删除服务
    log.push("  正在移除服务…".to_string());
    for name in MIPC_MANAGER_SERVICES {
        match remove_service(name) {
            Ok(true) => log.push(format!("    ✓ 已删除服务：{name}")),
            Ok(false) => {} // 服务不存在，跳过
            Err(e) => log.push(format!("    ⚠ 删除服务 {name} 失败：{e}")),
        }
    }

    // 等待后台卸载子进程完成文件清理
    std::thread::sleep(Duration::from_secs(2));

    // 5. 清理目录
    log.push("  正在清理文件…".to_string());
    cleanup_product_directories(log, true);

    log.push("✓ 小米电脑管家卸载完成".to_string());
    Ok(())
}

/// 卸载小米互联 / 互联互通（PcContinuity / HyperConnect，完整流程）。
pub fn uninstall_pc_continuity(root: &Path, log: &mut Vec<String>) -> Result<()> {
    log.push(format!(
        "开始卸载小米互联 / 互联互通（HyperConnect / PcContinuity）：{}",
        root.display()
    ));

    // 1. 找到版本目录并运行 uninstall.exe
    let version = crate::install::latest_version_dir(root)?;
    let uninstall_exe = version.join("uninstall.exe");
    log.push(format!("  正在运行卸载程序：{}", uninstall_exe.display()));
    let removed = run_product_uninstaller(&uninstall_exe)?;
    if !removed {
        bail!(
            "主程序卸载未完成，已停止服务和文件清理：{}",
            uninstall_exe.display()
        );
    }
    log.push("  ✓ 主程序卸载完成".to_string());

    // 2. 删除服务
    log.push("  正在移除服务…".to_string());
    for name in PC_CONTINUITY_SERVICES {
        match remove_service(name) {
            Ok(true) => log.push(format!("    ✓ 已删除服务：{name}")),
            Ok(false) => {}
            Err(e) => log.push(format!("    ⚠ 删除服务 {name} 失败：{e}")),
        }
    }

    // 等待后台卸载子进程完成文件清理
    std::thread::sleep(Duration::from_secs(2));

    // 3. 清理目录
    log.push("  正在清理文件…".to_string());
    cleanup_product_directories(log, false);

    log.push("✓ 小米互联 / 互联互通（HyperConnect / PcContinuity）卸载完成".to_string());
    Ok(())
}

/// 卸载子产品（AIService / MiService）。
fn uninstall_sub_product(log: &mut Vec<String>, root_path: &str, label: &str) {
    let root = Path::new(root_path);
    if !root.is_dir() {
        log.push(format!("  - {label} 未安装，跳过"));
        return;
    }
    match crate::install::latest_version_dir(root) {
        Ok(version) => {
            let exe = version.join("uninstall.exe");
            if exe.is_file() {
                log.push(format!("  正在卸载 {label}：{}", exe.display()));
                match run_product_uninstaller(&exe) {
                    Ok(true) => log.push(format!("    ✓ {label} 卸载完成")),
                    Ok(false) => {
                        log.push(format!("    ⚠ {label} 卸载程序未删除自身，卸载可能未完成"))
                    }
                    Err(e) => log.push(format!("    ⚠ 卸载 {label} 失败：{e}")),
                }
            } else {
                log.push(format!("    ⚠ {label} 目录存在但未找到 uninstall.exe"));
            }
        }
        Err(_) => {
            log.push(format!("    ⚠ {label} 目录存在但无版本子目录"));
        }
    }
}

/// 清理产品相关目录和临时文件。
fn cleanup_product_directories(log: &mut Vec<String>, is_manager: bool) {
    // C:\ProgramData\MI
    match remove_dir_if_exists(Path::new(r"C:\ProgramData\MI")) {
        Ok(true) => log.push("    ✓ 已删除 C:\\ProgramData\\MI".to_string()),
        Ok(false) => {}
        Err(e) => log.push(format!("    ⚠ 清理 C:\\ProgramData\\MI 失败：{e}")),
    }

    // %LOCALAPPDATA%\Temp\Timi Personal Computing\
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if !localappdata.is_empty() {
        let timi_temp = Path::new(&localappdata)
            .join("Temp")
            .join("Timi Personal Computing");

        let subdirs: &[&str] = if is_manager {
            &["MiServiceTMPX", "XiaomiPCManagerTMPX", "AIServiceTMPX"]
        } else {
            &["PcContinuityTMPX", "HyperConnectTMPX"]
        };

        for sub in subdirs {
            let dir = timi_temp.join(sub);
            match remove_dir_if_exists(&dir) {
                Ok(true) => log.push(format!("    ✓ 已删除 {}", dir.display())),
                Ok(false) => {}
                Err(e) => log.push(format!("    ⚠ 清理 {} 失败：{e}", dir.display())),
            }
        }

        // 如果 Timi Personal Computing 目录变空，也删除它
        if timi_temp.is_dir() {
            let _ = remove_dir_if_exists(&timi_temp);
        }
    }

    // 新版 HyperConnect 的更新器缓存目录（Electron app-update）。
    if !localappdata.is_empty() {
        let updater_cache = Path::new(&localappdata).join("miclaw-updater");
        match remove_dir_if_exists(&updater_cache) {
            Ok(true) => log.push(format!("    ✓ 已删除 {}", updater_cache.display())),
            Ok(false) => {}
            Err(e) => log.push(format!("    ⚠ 清理 {} 失败：{e}", updater_cache.display())),
        }
    }
}

/// 获取卸载描述文本（用于前端确认提示）。
pub fn uninstall_description() -> Result<String> {
    let manager_root = crate::install::find_install_root();
    let continuity_root = crate::install::find_pc_continuity_root();

    match (manager_root, continuity_root) {
        (Some(_), Some(_)) => {
            bail!("同时检测到小米电脑管家和小米互联。\n当前不支持同时安装，请逐一卸载。")
        }
        (Some(root), None) => Ok(format!(
            "将卸载 小米电脑管家\n\n安装目录：{}\n包含：主程序, AIService, MiService\n将删除所有相关服务与临时文件\n\n此操作不可逆！",
            root.display()
        )),
        (None, Some(root)) => Ok(format!(
            "将卸载 小米互联 / 互联互通（HyperConnect / PcContinuity）\n\n安装目录：{}\n将删除所有相关服务与临时文件\n\n此操作不可逆！",
            root.display()
        )),
        (None, None) => {
            bail!("未检测到已安装的小米电脑管家或小米互联")
        }
    }
}
