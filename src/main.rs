//! 小米电脑管家 / 小米互联 功能增强补丁工具（命令行前端）。

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use mipcmanager_patch::{
    elevate,
    experimental::{audio_dual_nic, smbios_spoof},
    i18n,
    install::pc_manager_installer,
    ops,
    patches::{device as device_spoof, xiaomi_share_menu},
    share_menu,
};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "MiPCM_CLI", about = "小米电脑管家功能增强补丁工具", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 查看安装信息与补丁状态
    Status,
    /// 地区伪装（micont_rtm.dll）
    Locale {
        #[command(subcommand)]
        action: PatchAction,
        /// 伪装地区代码（默认 CN）
        #[arg(long, default_value = "CN", global = true)]
        region: String,
        /// 不修改注册表
        #[arg(long, global = true)]
        no_registry: bool,
    },
    /// 抑制「请确认摄像头状态」弹窗（PcControlCenter.dll）
    Camera {
        #[command(subcommand)]
        action: PatchAction,
    },
    /// MiPCAudio 音频流转广播模式（无线/有线，统一身份并修复双网卡媒体路由）
    Audio {
        #[command(subcommand)]
        action: AudioAction,
    },
    /// 设备伪装（释放 msimg32.dll + 写入机型注册表）
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// [实验性] SMBIOS 设备身份伪装 — 妙播 / Lyra 特殊适配（micont_rtm.dll IAT Hook）
    Smbios {
        #[command(subcommand)]
        action: SmbiosAction,
    },
    /// Windows 11 一级右键“使用小米互传发送”
    ShareMenu {
        #[command(subcommand)]
        action: ShareMenuAction,
    },
    /// 安装小米电脑管家 / 小米互联（自动识别安装包所属产品）
    Install {
        /// 显式指定 .exe 安装包
        #[arg(long, value_name = "EXE", conflicts_with = "url")]
        installer: Option<PathBuf>,
        /// 从 HTTP(S) 地址下载安装包
        #[arg(long, value_name = "URL", conflicts_with = "installer")]
        url: Option<String>,
        /// 下载内置版本：manager、continuity 或 hyperconnect-beta；默认 manager
        #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "manager", conflicts_with_all = ["installer", "url"])]
        recommended: Option<RecommendedArg>,
    },
    /// 安装或维护超级小爱（userenv.dll）
    Xiaoai {
        #[command(subcommand)]
        action: XiaoaiAction,
    },
    /// 卸载 MiDrop Ext MSIX 包 或 小米电脑管家 / 小米互联
    Uninstall {
        #[command(subcommand)]
        action: UninstallAction,
    },
}

#[derive(Subcommand, Clone)]
enum PatchAction {
    /// 应用补丁
    Apply {
        /// 指定目标 DLL 路径（默认自动探测安装目录）
        #[arg(long)]
        dll: Option<PathBuf>,
        /// 不自动关闭相关进程
        #[arg(long)]
        no_kill: bool,
    },
    /// 还原补丁
    Revert {
        #[arg(long)]
        dll: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
}

#[derive(Subcommand, Clone)]
enum ShareMenuAction {
    /// 启用右键菜单
    Apply,
    /// 关闭右键菜单并清理组件
    Revert,
}

#[derive(Subcommand, Clone)]
enum AudioAction {
    /// 切换广播模式
    Apply {
        /// 广播介质：wifi（无线，默认）或 lan（有线）
        #[arg(long, value_enum, default_value_t = ModeArg::Wifi)]
        mode: ModeArg,
        /// 指定版本目录（默认自动探测）
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
        /// 不自动管理 Wi-Fi 本地子网优先路由（无线模式下用于修复双网卡同网段断流）
        #[arg(long)]
        no_wifi_local_route: bool,
    },
    /// 还原音频流转补丁
    Revert {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
    /// [实验性] 双网卡同网段音频修复：诊断并自动对齐 Wi-Fi 路由与广播模式
    ExperimentalFix {
        /// 仅诊断，不执行修复
        #[arg(long)]
        dry_run: bool,
        /// 指定版本目录（默认自动探测）
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Clone)]
enum DeviceAction {
    /// 应用设备伪装
    Apply {
        /// 机型代号（默认 TM2425）
        #[arg(long, default_value = device_spoof::DEFAULT_MODEL)]
        model: String,
        /// 指定版本目录（默认自动探测）
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
    /// 还原设备伪装
    Revert {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
}

#[derive(Subcommand, Clone)]
enum SmbiosAction {
    /// 应用 SMBIOS 设备身份伪装
    Apply {
        /// 机型代号（默认 TM2425）
        #[arg(long, default_value = smbios_spoof::DEFAULT_MODEL)]
        model: String,
        /// 指定目标 DLL 路径（默认自动探测安装目录）
        #[arg(long)]
        dll: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
    /// 还原 SMBIOS 设备身份伪装
    Revert {
        #[arg(long)]
        dll: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
}

#[derive(Subcommand, Clone)]
enum XiaoaiAction {
    /// 安装超级小爱，等待安装器结束后自动注入运行补丁
    Install {
        /// 显式指定 .exe 安装包
        #[arg(long, value_name = "EXE", conflicts_with = "url")]
        installer: Option<PathBuf>,
        /// 从 HTTP(S) 地址下载安装包
        #[arg(long, value_name = "URL", conflicts_with = "installer")]
        url: Option<String>,
        /// 下载并安装内置版超级小爱
        #[arg(long, conflicts_with_all = ["installer", "url"])]
        recommended: bool,
    },
    /// 向已安装的最新版本目录应用补丁
    Apply {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
    /// 还原已安装目录中的补丁
    Revert {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        no_kill: bool,
    },
}

#[derive(Subcommand, Clone)]
enum UninstallAction {
    /// 卸载 MiDrop Ext MSIX 包（完成后重启资源管理器）
    Msix,
    /// 完整卸载小米电脑管家 / 小米互联（主程序 + 子产品 + 服务 + 文件清理，不可逆）
    Product,
}

#[derive(ValueEnum, Clone, Copy)]
enum ModeArg {
    Wifi,
    Lan,
}

#[derive(ValueEnum, Clone, Copy)]
enum RecommendedArg {
    Manager,
    Continuity,
    HyperconnectBeta,
}

impl From<RecommendedArg> for ops::RecommendedInstaller {
    fn from(value: RecommendedArg) -> Self {
        match value {
            RecommendedArg::Manager => Self::PcManager,
            RecommendedArg::Continuity => Self::PcContinuity,
            RecommendedArg::HyperconnectBeta => Self::HyperConnectBeta,
        }
    }
}

impl From<ModeArg> for ops::BroadcastMode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Wifi => ops::BroadcastMode::Wireless,
            ModeArg::Lan => ops::BroadcastMode::Wired,
        }
    }
}

fn main() {
    elevate::ensure_elevated();
    let lang = i18n::detect_lang();
    if let Some(message) = ops::close_all_on_startup() {
        println!("{message}");
    }

    let cli = Cli::parse();
    let result = match cli.command {
        Some(cmd) => run(cmd, lang),
        #[cfg(feature = "cli")]
        None => mipcmanager_patch::ui::tui::run(),
        #[cfg(not(feature = "cli"))]
        None => {
            eprintln!("TUI 未编译（需启用 cli feature），请使用子命令: status / install / ...");
            std::process::exit(1);
        }
    };
    if let Err(e) = result {
        eprintln!(
            "{}",
            i18n::tr("cli.error", lang).replace("{error}", &format!("{e:#}"))
        );
        std::process::exit(1);
    }
}

fn run(cmd: Command, lang: i18n::Lang) -> Result<()> {
    match cmd {
        Command::Status => {
            let mut lines = ops::status_lines();
            let share_state = match xiaomi_share_menu::current_state() {
                xiaomi_share_menu::ShellMenuState::Enabled => "已启用",
                xiaomi_share_menu::ShellMenuState::Disabled => "未启用",
                xiaomi_share_menu::ShellMenuState::Partial => "状态不完整（可重新应用修复）",
            };
            lines.push(format!("Windows 11 右键小米互传: {share_state}"));
            print_log(lines);
            Ok(())
        }
        Command::Locale {
            action,
            region,
            no_registry,
        } => match action {
            PatchAction::Apply { dll, no_kill } => {
                print_log(ops::apply_locale(dll, &region, !no_registry, no_kill)?);
                Ok(())
            }
            PatchAction::Revert { dll, no_kill } => {
                print_log(ops::revert_locale(dll, !no_registry, no_kill)?);
                Ok(())
            }
        },
        Command::Camera { action } => match action {
            PatchAction::Apply { dll, no_kill } => {
                print_log(ops::apply_camera(dll, no_kill)?);
                Ok(())
            }
            PatchAction::Revert { dll, no_kill } => {
                print_log(ops::revert_camera(dll, no_kill)?);
                Ok(())
            }
        },
        Command::Audio { action } => match action {
            AudioAction::Apply {
                mode,
                dir,
                no_kill,
                no_wifi_local_route,
            } => {
                print_log(ops::apply_audio_with_options(
                    mode.into(),
                    dir,
                    no_kill,
                    no_wifi_local_route,
                )?);
                Ok(())
            }
            AudioAction::Revert { dir, no_kill } => {
                print_log(ops::revert_audio(dir, no_kill)?);
                Ok(())
            }
            AudioAction::ExperimentalFix { dry_run, dir } => {
                let dir = dir.map(Ok).unwrap_or_else(ops::resolve_full_version_dir)?;
                if dry_run {
                    print_log(audio_dual_nic::diagnose(&dir)?);
                } else {
                    print_log(audio_dual_nic::auto_fix(&dir)?);
                }
                Ok(())
            }
        },
        Command::Device { action } => match action {
            DeviceAction::Apply {
                model,
                dir,
                no_kill,
            } => {
                print_log(ops::apply_device(&model, dir, no_kill)?);
                Ok(())
            }
            DeviceAction::Revert { dir, no_kill } => {
                print_log(ops::revert_device(dir, no_kill)?);
                Ok(())
            }
        },
        Command::Smbios { action } => match action {
            SmbiosAction::Apply {
                model,
                dll,
                no_kill,
            } => {
                print_log(ops::apply_smbios(Some(&model), dll, no_kill)?);
                Ok(())
            }
            SmbiosAction::Revert { dll, no_kill } => {
                print_log(ops::revert_smbios(dll, no_kill)?);
                Ok(())
            }
        },
        Command::ShareMenu { action } => match action {
            ShareMenuAction::Apply => {
                print_log(share_menu::apply()?);
                Ok(())
            }
            ShareMenuAction::Revert => {
                print_log(share_menu::revert()?);
                Ok(())
            }
        },
        Command::Install {
            installer,
            url,
            recommended,
        } => install_pc_manager(installer, url, recommended, lang),
        Command::Xiaoai { action } => match action {
            XiaoaiAction::Install {
                installer,
                url,
                recommended,
            } => install_xiaoai(installer, url, recommended, lang),
            XiaoaiAction::Apply { dir, no_kill } => {
                print_log(ops::apply_xiaoai(dir, no_kill)?);
                Ok(())
            }
            XiaoaiAction::Revert { dir, no_kill } => {
                print_log(ops::revert_xiaoai(dir, no_kill)?);
                Ok(())
            }
        },
        Command::Uninstall { action } => match action {
            UninstallAction::Msix => {
                print_log(ops::uninstall_msix(false)?);
                Ok(())
            }
            UninstallAction::Product => {
                let desc = ops::uninstall_product_description()?;
                println!("{desc}");
                println!();
                let input = prompt(i18n::tr("cli.confirm.uninstall", lang))?;
                if input.to_lowercase() != "y" {
                    println!("{}", i18n::tr("cli.cancelled.uninstall", lang));
                    return Ok(());
                }
                print_log(ops::uninstall_product()?);
                Ok(())
            }
        },
    }
}

fn print_log(lines: Vec<String>) {
    for line in lines {
        println!("{line}");
    }
}

// ===================== 安装（交互式选择安装包） =====================

fn install_pc_manager(
    explicit: Option<PathBuf>,
    url: Option<String>,
    recommended: Option<RecommendedArg>,
    lang: i18n::Lang,
) -> Result<()> {
    let patcher_dir = pc_manager_installer::patcher_dir()?;
    let source = if let Some(product) = recommended {
        Some(InstallerSource::Recommended(product.into()))
    } else {
        choose_manager_installer(explicit, url, &patcher_dir, lang)?
    };
    let Some(source) = source else {
        println!("{}", i18n::tr("cli.cancelled.install", lang));
        return Ok(());
    };
    let control = ops::DownloadControl::default();
    let mut progress = console_download_progress(lang);
    let result = match source {
        InstallerSource::Local(installer) => ops::install_from_path(&installer),
        InstallerSource::Url(url) => {
            ops::download_and_install_pc_manager(Some(&url), &control, &mut progress)
        }
        InstallerSource::Recommended(product) => {
            println!("{}\n{}", product.label(lang), product.requirement(lang));
            ops::download_and_install_recommended(product, &control, &mut progress)
        }
    };
    eprintln!();
    print_log(result?);
    Ok(())
}

fn install_xiaoai(
    explicit: Option<PathBuf>,
    url: Option<String>,
    recommended: bool,
    lang: i18n::Lang,
) -> Result<()> {
    let control = ops::DownloadControl::default();
    let mut progress = console_download_progress(lang);
    let result = if recommended {
        ops::download_and_install_recommended(
            ops::RecommendedInstaller::Xiaoai,
            &control,
            &mut progress,
        )
    } else {
        match (explicit, url) {
            (Some(installer), None) => ops::install_xiaoai_from_path(&installer),
            (None, Some(url)) => ops::download_and_install_xiaoai(&url, &control, &mut progress),
            (None, None) => ops::install_local_xiaoai(),
            (Some(_), Some(_)) => unreachable!("clap rejects conflicting installer sources"),
        }
    };
    eprintln!();
    print_log(result?);
    Ok(())
}

fn console_download_progress(lang: i18n::Lang) -> impl FnMut(ops::DownloadProgress) {
    use std::io::{IsTerminal, Write};
    let terminal = std::io::stderr().is_terminal();
    let mut previous_phase = None;
    move |progress| {
        if terminal {
            let mut stderr = std::io::stderr();
            let _ = crossterm::execute!(
                stderr,
                crossterm::cursor::MoveToColumn(0),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
            );
            eprint!("{}", ops::download_progress_text(progress, lang));
            let _ = stderr.flush();
        } else if previous_phase != Some(progress.phase) {
            eprintln!("{}", ops::download_progress_text(progress, lang));
        }
        previous_phase = Some(progress.phase);
    }
}

enum InstallerSource {
    Recommended(ops::RecommendedInstaller),
    Url(String),
    Local(PathBuf),
}

fn choose_manager_installer(
    explicit: Option<PathBuf>,
    url: Option<String>,
    patcher_dir: &Path,
    lang: i18n::Lang,
) -> Result<Option<InstallerSource>> {
    if let Some(path) = explicit {
        return Ok(Some(InstallerSource::Local(path)));
    }
    if let Some(url) = url {
        return Ok(Some(InstallerSource::Url(url)));
    }

    let candidates = pc_manager_installer::find_local_installers(patcher_dir)?;
    match candidates.as_slice() {
        [only] => {
            let kind = pc_manager_installer::classify_installer(only);
            println!(
                "{}",
                i18n::tr("cli.found.installer", lang)
                    .replace("{kind}", kind.label_for(lang))
                    .replace("{path}", &only.display().to_string())
            );
            Ok(Some(InstallerSource::Local(only.clone())))
        }
        [] => prompt_installer_source(lang),
        _ => prompt_installer_candidate(&candidates, lang)
            .map(|selected| selected.map(InstallerSource::Local)),
    }
}

// ── 数据驱动的安装包选择菜单 ────────────────────────────────────

enum InstallerSourceAction {
    Recommended(ops::RecommendedInstaller),
    DownloadUrl,
    SpecifyPath,
}

struct MenuEntry {
    key: &'static str,
    description: &'static str,
    action: InstallerSourceAction,
}

const INSTALLER_SOURCE_MENU: &[MenuEntry] = &[
    MenuEntry {
        key: "1",
        description: "install.source.manager",
        action: InstallerSourceAction::Recommended(ops::RecommendedInstaller::PcManager),
    },
    MenuEntry {
        key: "2",
        description: "install.source.continuity",
        action: InstallerSourceAction::Recommended(ops::RecommendedInstaller::PcContinuity),
    },
    MenuEntry {
        key: "3",
        description: "install.source.hyperconnect-beta",
        action: InstallerSourceAction::Recommended(ops::RecommendedInstaller::HyperConnectBeta),
    },
    MenuEntry {
        key: "4",
        description: "cli.menu.download_url",
        action: InstallerSourceAction::DownloadUrl,
    },
    MenuEntry {
        key: "5",
        description: "cli.menu.local_file",
        action: InstallerSourceAction::SpecifyPath,
    },
];

fn prompt_installer_candidate(candidates: &[PathBuf], lang: i18n::Lang) -> Result<Option<PathBuf>> {
    println!("{}", i18n::tr("cli.multiple.installers", lang));
    for (index, path) in candidates.iter().enumerate() {
        let kind = pc_manager_installer::classify_installer(path);
        println!(
            "  {}) [{}] {}",
            index + 1,
            kind.label_for(lang),
            path.display()
        );
    }
    println!("  0) {}", i18n::tr("cli.cancel.option", lang));
    let choice = prompt(i18n::tr("cli.choose.installer", lang))?;
    if choice == "0" || choice.is_empty() {
        return Ok(None);
    }
    let index = choice
        .parse::<usize>()
        .ok()
        .filter(|index| (1..=candidates.len()).contains(index))
        .context(i18n::tr("cli.invalid.choice", lang))?;
    Ok(Some(candidates[index - 1].clone()))
}

fn prompt_installer_source(lang: i18n::Lang) -> Result<Option<InstallerSource>> {
    println!("{}", i18n::tr("cli.no.installer.found", lang));
    for entry in INSTALLER_SOURCE_MENU {
        println!("  {}) {}", entry.key, i18n::tr(entry.description, lang));
    }
    println!("  0) {}", i18n::tr("cli.cancel.option", lang));
    let choice = prompt(i18n::tr("cli.choose.option", lang))?;
    let action = INSTALLER_SOURCE_MENU
        .iter()
        .find(|e| e.key == choice)
        .map(|e| &e.action);
    match action {
        Some(InstallerSourceAction::Recommended(product)) => {
            Ok(Some(InstallerSource::Recommended(*product)))
        }
        Some(InstallerSourceAction::DownloadUrl) => {
            let url = prompt(i18n::tr("cli.prompt.url", lang))?;
            if url.is_empty() {
                return Ok(None);
            }
            Ok(Some(InstallerSource::Url(url)))
        }
        Some(InstallerSourceAction::SpecifyPath) => {
            let input = prompt(i18n::tr("cli.prompt.path", lang))?;
            let path = input.trim().trim_matches(['"', '\'']);
            if path.is_empty() {
                Ok(None)
            } else {
                Ok(Some(InstallerSource::Local(PathBuf::from(path))))
            }
        }
        None if choice == "0" || choice.is_empty() => Ok(None),
        None => bail!(i18n::tr("cli.invalid.selection", lang)),
    }
}

fn prompt(msg: &str) -> Result<String> {
    use std::io::Write;
    print!("{msg}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod install_routing_tests {
    use super::*;

    #[test]
    fn recommended_cli_routes_each_manager_variant() {
        for (arguments, expected) in [
            (
                vec!["MiPCM_CLI", "install", "--recommended"],
                ops::RecommendedInstaller::PcManager,
            ),
            (
                vec!["MiPCM_CLI", "install", "--recommended", "continuity"],
                ops::RecommendedInstaller::PcContinuity,
            ),
            (
                vec!["MiPCM_CLI", "install", "--recommended", "hyperconnect-beta"],
                ops::RecommendedInstaller::HyperConnectBeta,
            ),
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            let Some(Command::Install {
                recommended: Some(product),
                ..
            }) = cli.command
            else {
                panic!("recommended source was not selected");
            };
            assert_eq!(ops::RecommendedInstaller::from(product), expected);
        }
        assert!(Cli::try_parse_from(["MiPCM_CLI", "install", "--recommended", "unknown"]).is_err());
    }

    #[test]
    fn share_menu_command_parses() {
        assert!(Cli::try_parse_from(["MiPCM_CLI", "share-menu", "apply"]).is_ok());
        assert!(Cli::try_parse_from(["MiPCM_CLI", "share-menu", "revert"]).is_ok());
    }

    #[test]
    fn xiaoai_recommended_conflicts_with_manual_sources() {
        assert!(Cli::try_parse_from(["MiPCM_CLI", "xiaoai", "install", "--recommended"]).is_ok());
        for args in [
            vec![
                "MiPCM_CLI",
                "xiaoai",
                "install",
                "--recommended",
                "--url",
                "https://example.com/a.exe",
            ],
            vec![
                "MiPCM_CLI",
                "xiaoai",
                "install",
                "--recommended",
                "--installer",
                "local.exe",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn install_cli_accepts_exactly_one_package_source() {
        assert!(Cli::try_parse_from(["MiPCM_CLI", "install"]).is_ok());
        assert!(Cli::try_parse_from(["MiPCM_CLI", "install", "--recommended"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "MiPCM_CLI",
                "install",
                "--recommended",
                "--installer",
                "local.exe"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "MiPCM_CLI",
                "install",
                "--recommended",
                "--url",
                "https://example.com/a.exe"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["MiPCM_CLI", "install", "--installer", "XiaomiPCManager.exe"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "MiPCM_CLI",
                "install",
                "--url",
                "https://example.com/XiaomiPCManager.exe"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "MiPCM_CLI",
                "install",
                "--installer",
                "local.exe",
                "--url",
                "https://example.com/remote.exe"
            ])
            .is_err()
        );
    }
}
