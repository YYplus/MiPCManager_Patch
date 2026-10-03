//! 超级小爱安装器启动、等待和安装目录定位。

use crate::infra::download::{self, DownloadControl, DownloadProgress};
use crate::install::pc_manager_installer;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const DOWNLOAD_FALLBACK_NAME: &str = "XiaoaiAgent_Setup.exe";

/// 查找 Patcher 同目录下的超级小爱安装包。
///
/// 安装包文件名并不可靠地表达最终安装版本，版本只在安装完成后通过目录校验。
pub fn find_local_installers(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut installers = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("无法读取目录 {}", dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if is_installer_filename(&name) {
            installers.push(entry.path());
        }
    }
    installers.sort();
    Ok(installers)
}

/// 下载超级小爱安装包。
pub fn download_installer(
    url: &str,
    target_dir: &Path,
    control: &DownloadControl,
    progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf> {
    let downloaded_name = pc_manager_installer::download_filename(url)?;
    let needs_xiaoai_fallback = downloaded_name == "XiaomiPCManagerInstaller.exe";
    let filename = if needs_xiaoai_fallback {
        DOWNLOAD_FALLBACK_NAME
    } else {
        &downloaded_name
    };
    download::download(url, &target_dir.join(filename), None, control, progress)
}

/// 返回安装根目录下版本号最高的目录，不限制具体版本。
pub fn latest_version_dir(root: &Path) -> Result<PathBuf> {
    crate::install::latest_version_dir(root)
        .with_context(|| format!("无法确定超级小爱版本目录：{}", root.display()))
}

/// 校验显式指定的超级小爱版本目录是否存在。
pub fn ensure_version_dir(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        bail!("指定的超级小爱版本目录不存在：{}", dir.display());
    }
    Ok(())
}

/// 启动安装器并等待安装窗口对应进程退出。
pub fn launch_installer_and_wait(installer: &Path) -> Result<()> {
    validate_installer(installer)?;
    let installer = installer
        .canonicalize()
        .with_context(|| format!("无法解析安装包路径 {}", installer.display()))?;
    let parent = installer.parent().context("无法确定安装包所在目录")?;
    let status = wait_for_installer_exit(&installer, parent)?;
    if !status.success() {
        bail!("超级小爱安装程序未成功完成（退出码：{status}）");
    }
    Ok(())
}

#[cfg(windows)]
fn wait_for_installer_exit(installer: &Path, parent: &Path) -> Result<ExitStatus> {
    use std::collections::HashSet;
    use std::thread;
    use std::time::Duration;
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut child = Command::new(installer)
        .current_dir(parent)
        .spawn()
        .with_context(|| format!("无法启动超级小爱安装包 {}", installer.display()))?;
    let root_pid = Pid::from_u32(child.id());
    let mut known_installer_pids = HashSet::from([root_pid]);
    let mut direct_status = None;
    let mut quiet_observations = 0;
    let mut system = System::new();

    loop {
        system.refresh_processes(ProcessesToUpdate::All, true);
        loop {
            let descendants: Vec<Pid> = system
                .processes()
                .iter()
                .filter_map(|(pid, process)| {
                    let parent_pid = process.parent()?;
                    (known_installer_pids.contains(&parent_pid)
                        && !is_installed_xiaoai_process(process.exe()))
                    .then_some(*pid)
                })
                .collect();
            let before = known_installer_pids.len();
            known_installer_pids.extend(descendants);
            if known_installer_pids.len() == before {
                break;
            }
        }

        if direct_status.is_none() {
            direct_status = child.try_wait().context("等待超级小爱安装程序退出时出错")?;
        }
        let descendant_is_running = known_installer_pids
            .iter()
            .any(|pid| *pid != root_pid && system.process(*pid).is_some());
        if let Some(status) = direct_status.as_ref().filter(|_| !descendant_is_running) {
            quiet_observations += 1;
            if quiet_observations >= 3 {
                return Ok(*status);
            }
        } else {
            quiet_observations = 0;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(windows)]
fn is_installed_xiaoai_process(executable: Option<&Path>) -> bool {
    let Some(executable) = executable else {
        return false;
    };
    let executable = normalize_path(executable);
    let mut roots = vec![PathBuf::from(crate::install::DEFAULT_XIAOAI_ROOT)];
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        roots.push(Path::new(&program_files).join("MI").join("XiaoaiAgent"));
    }
    roots.into_iter().any(|root| {
        let root = normalize_path(&root);
        executable == root || executable.starts_with(&format!("{root}\\"))
    })
}

#[cfg(windows)]
fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(not(windows))]
fn wait_for_installer_exit(installer: &Path, parent: &Path) -> Result<ExitStatus> {
    Command::new(installer)
        .current_dir(parent)
        .status()
        .with_context(|| format!("无法启动超级小爱安装包 {}", installer.display()))
}

fn validate_installer(installer: &Path) -> Result<()> {
    if !installer.is_file() {
        bail!("指定的超级小爱安装包不存在：{}", installer.display());
    }
    if !installer
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        bail!("超级小爱安装包必须是 .exe 文件：{}", installer.display());
    }
    let name = installer
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !is_installer_filename(name) {
        bail!(
            "不支持的超级小爱安装包名称：{}；仅识别 XiaoaiAgent_Setup.exe 或 s6bK_XiaoaiAgent_3.5.0.220_31444585.exe",
            installer.display()
        );
    }
    Ok(())
}

fn is_installer_filename(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "xiaoaiagent_setup.exe" | "s6bk_xiaoaiagent_3.5.0.220_31444585.exe"
    )
}
