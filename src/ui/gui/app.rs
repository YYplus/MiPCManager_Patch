#![windows_subsystem = "windows"]

use anyhow::{Context, Result, bail};
use mipcmanager_patch::{
    elevate,
    experimental::smbios_spoof,
    i18n,
    infra::pe::PeImage,
    install,
    ops,
    patches::{ai, audio, camera, device as ds, locale},
    uninstall,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
#[cfg(windows)]
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

slint::include_modules!();

#[cfg(not(windows))]
fn main() {
    eprintln!("MiPCM_GUI 仅支持 Windows。");
}

#[cfg(windows)]
fn main() {
    elevate::ensure_elevated();

    if let Some(message) = ops::close_all_on_startup() {
        let _ = message;
    }

    let lang = i18n::detect_lang();
    let app = AppWindow::new().unwrap();

    app.on_tr(move |key: SharedString| -> SharedString {
        let key = key.to_string();
        match (key.as_str(), lang) {
            ("install.one-click", i18n::Lang::Zh) => "一键安装".into(),
            ("install.one-click", i18n::Lang::En) => "One-click Install".into(),
            ("install.row.manager", i18n::Lang::Zh) => "小米电脑管家".into(),
            ("install.row.manager", i18n::Lang::En) => "MiPCManager".into(),
            ("install.row.continuity", i18n::Lang::Zh) => "小米互联".into(),
            ("install.row.continuity", i18n::Lang::En) => "Xiaomi Interconnectivity".into(),
            ("install.row.xiaoai", i18n::Lang::Zh) => "超级小爱".into(),
            ("install.row.xiaoai", i18n::Lang::En) => "Super XiaoAI".into(),
            ("state.applied", i18n::Lang::Zh) => "已应用".into(),
            ("state.applied", i18n::Lang::En) => "Applied".into(),
            ("state.not-applied", i18n::Lang::Zh) => "未应用".into(),
            ("state.not-applied", i18n::Lang::En) => "Not applied".into(),
            ("state.fixed", i18n::Lang::Zh) => "已修复".into(),
            ("state.fixed", i18n::Lang::En) => "Fixed".into(),
            ("state.needs-fix", i18n::Lang::Zh) => "待修复".into(),
            ("state.needs-fix", i18n::Lang::En) => "Needs fix".into(),
            ("state.not-configured", i18n::Lang::Zh) => "未配置".into(),
            ("state.not-configured", i18n::Lang::En) => "Not configured".into(),
            ("state.unknown", i18n::Lang::Zh) => "状态异常".into(),
            ("state.unknown", i18n::Lang::En) => "Unknown".into(),
            _ => i18n::tr(&key, lang).into(),
        }
    });

    let sources = ops::RecommendedInstaller::MANAGER_VARIANTS;
    app.set_manager_sources(ModelRc::new(VecModel::from(
        sources
            .map(|source| SharedString::from(source.label(lang)))
            .to_vec(),
    )));
    app.set_manager_requirements(ModelRc::new(VecModel::from(
        sources
            .map(|source| SharedString::from(source.requirement(lang)))
            .to_vec(),
    )));
    app.set_xiaoai_source(ops::RecommendedInstaller::Xiaoai.label(lang).into());

    let presets: Vec<SharedString> = ds::PRESETS
        .iter()
        .map(|p| SharedString::from(format!("{} · {}", p.code, p.name)))
        .collect();
    app.set_model_presets(ModelRc::from(Rc::new(VecModel::from(presets))));

    let codes: Vec<SharedString> = ds::PRESETS
        .iter()
        .map(|p| SharedString::from(p.code))
        .collect();
    app.set_model_codes(ModelRc::from(Rc::new(VecModel::from(codes))));

    refresh(&app);
    setup_callbacks(&app, lang);
    app.run().unwrap();
}

fn current_locale_dll() -> Option<PathBuf> {
    if let Some(root) = install::find_install_root() {
        let version = install::latest_version_dir(&root).ok()?;
        return Some(version.join(locale::TARGET_DLL));
    }
    let root = install::find_pc_continuity_root()?;
    let version = install::latest_version_dir(&root).ok()?;
    Some(install::runtime_native_dir(&version).join(locale::TARGET_DLL))
}

fn locale_is_patched(path: &Path) -> bool {
    let Ok(mut data) = fs::read(path) else {
        return false;
    };
    matches!(
        locale::patch_bytes(&mut data),
        Ok(locale::PatchOutcome::AlreadyPatched)
    )
}

fn differs_from_backup(path: &Path) -> bool {
    let backup = install::backup_path(path);
    let (Ok(current), Ok(original)) = (fs::read(path), fs::read(backup)) else {
        return false;
    };
    current != original
}

fn audio_mode(version: &Path) -> i32 {
    let states = audio::current_state(version);
    if states.is_empty() {
        return 0;
    }
    let all_wifi = states
        .iter()
        .all(|(_, state)| state.contains("无线") || state.contains("WiFi"));
    if all_wifi {
        return 1;
    }
    let all_lan = states
        .iter()
        .all(|(_, state)| state.contains("有线") || state.contains("LAN"));
    if all_lan { 2 } else { 3 }
}

fn audio_route_active(version: &Path) -> bool {
    let state = audio::wifi_route_state(version);
    !state.contains("未配置") && !state.contains("状态不可读")
}

fn smbios_is_patched(path: &Path) -> bool {
    let Ok(data) = fs::read(path) else {
        return false;
    };
    let Ok(pe) = PeImage::parse(data) else {
        return false;
    };
    let Ok((_, iat_rva, _)) = pe.find_iat_entry("kernel32", "GetSystemFirmwareTable") else {
        return false;
    };
    install::backup_path(path).exists() && pe.find_call_to_iat(iat_rva).is_err()
}

fn sync_device_model(app: &AppWindow, model: &str) {
    if let Some(index) = ds::PRESETS.iter().position(|preset| preset.code == model) {
        app.set_custom_mode(false);
        app.set_model_idx(index as i32);
    } else {
        app.set_custom_mode(true);
        app.set_custom_model_input(model.into());
    }
}

fn refresh(app: &AppWindow) {
    let full = ops::full_features_available();
    let continuity = install::find_pc_continuity_root().is_some();
    let xiaoai_available = ops::xiaoai_available();
    app.set_full_features(full);
    app.set_continuity_available(continuity);
    app.set_xiaoai_available(xiaoai_available);
    app.set_status_text(ops::status_lines().join("\n").into());

    let locale_active = current_locale_dll()
        .as_deref()
        .is_some_and(locale_is_patched);
    app.set_locale_active(locale_active);

    app.set_device_active(false);
    app.set_camera_active(false);
    app.set_audio_active(false);
    app.set_audio_mode(0);
    app.set_dual_nic_state(0);
    app.set_smbios_active(false);

    if full && let Ok(version) = ops::resolve_full_version_dir() {
        let (proxy_ok, model) = ds::current_state(&version);
        let device_active = proxy_ok || model.is_some();
        app.set_device_active(device_active);
        if let Some(model) = model.as_deref() {
            sync_device_model(app, model);
        }

        let camera_path = version.join(camera::TARGET_DLL);
        app.set_camera_active(differs_from_backup(&camera_path));

        let mode = audio_mode(&version);
        let route_active = audio_route_active(&version);
        let audio_changed = [audio::TARGET_MIPCAUDIO, audio::TARGET_IDMRUNTIME]
            .iter()
            .any(|name| differs_from_backup(&version.join(name)));
        let audio_active = audio_changed || route_active;
        app.set_audio_active(audio_active);
        app.set_audio_mode(mode);
        let dual_state = if !audio_active {
            0
        } else {
            match mode {
                1 if route_active => 1,
                1 => 2,
                2 if !route_active => 1,
                2 => 2,
                _ => 3,
            }
        };
        app.set_dual_nic_state(dual_state);

        let smbios_path = version.join(smbios_spoof::TARGET_DLL);
        app.set_smbios_active(smbios_is_patched(&smbios_path));
    }

    let xiaoai_patch_active = install::find_xiaoai_root()
        .and_then(|root| install::latest_version_dir(&root).ok())
        .is_some_and(|version| ai::current_state(&version));
    app.set_xiaoai_patch_active(xiaoai_patch_active);
}

fn append_log(app: &AppWindow, label: &str, result: Result<Vec<String>>) {
    let mut log = format!("—— {} ——\n", label);
    match result {
        Ok(lines) => {
            app.set_last_error("".into());
            for line in lines {
                log.push_str(&line);
                log.push('\n');
            }
        }
        Err(e) => {
            let m = format!("{e:#}");
            log.push_str(&format!("Error: {m}\n"));
            app.set_last_error(m.into());
        }
    }
    let current: String = app.get_log_text().into();
    app.set_log_text(format!("{current}{log}").into());
    app.set_log_viewport_y(-100000.0);
    refresh(app);
}

fn run_patch(app: &AppWindow, label: &str, f: impl FnOnce() -> Result<Vec<String>>) {
    append_log(app, label, f());
}

fn ensure_manual_installer_kind(
    installer: &Path,
    expected: install::pc_manager_installer::InstallerKind,
) -> Result<()> {
    let name = installer
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let Some(actual) = install::pc_manager_installer::classify_installer_filename(name) else {
        bail!(
            "无法从文件名识别安装包类型：{}；请使用对应产品的官方安装包文件名",
            installer.display()
        );
    };
    if actual != expected {
        bail!(
            "安装包类型不匹配：当前入口仅用于{}，实际识别为{}",
            expected.label(),
            actual.label()
        );
    }
    Ok(())
}

fn ensure_manual_url_kind(
    url: &str,
    expected: install::pc_manager_installer::InstallerKind,
) -> Result<()> {
    let filename = install::pc_manager_installer::download_filename(url)?;
    ensure_manual_installer_kind(Path::new(&filename), expected)
}

fn xiaoai_uninstall_description() -> Result<String> {
    let root = install::find_xiaoai_root().context("未检测到已安装的超级小爱")?;
    Ok(format!(
        "将卸载 超级小爱\n\n安装目录：{}\n\n此操作不可逆！",
        root.display()
    ))
}

fn uninstall_xiaoai() -> Result<Vec<String>> {
    let root = install::find_xiaoai_root().context("未检测到已安装的超级小爱")?;
    let version = install::latest_version_dir(&root)?;
    let uninstall_exe = version.join("uninstall.exe");
    let mut log = vec![format!("开始卸载超级小爱：{}", root.display())];
    log.push(format!("  正在运行卸载程序：{}", uninstall_exe.display()));
    let removed = uninstall::run_product_uninstaller(&uninstall_exe)?;
    if removed {
        log.push("  ✓ 主程序卸载完成".to_string());
        if uninstall::remove_dir_if_exists(&root)? {
            log.push(format!("  ✓ 已清理 {}", root.display()));
        }
    } else {
        log.push(format!(
            "  ⚠ 卸载程序未删除自身，卸载可能未完成：{}",
            uninstall_exe.display()
        ));
    }
    log.push("✓ 超级小爱卸载流程完成".to_string());
    Ok(log)
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum InstallProduct {
    Manager,
    Xiaoai,
}

#[cfg(windows)]
fn spawn_install_operation(
    app_weak: slint::Weak<AppWindow>,
    label: String,
    product: InstallProduct,
    lang: i18n::Lang,
    operation: impl FnOnce(
        &ops::DownloadControl,
        &mut dyn FnMut(ops::DownloadProgress),
    ) -> Result<Vec<String>>
    + Send
    + 'static,
) -> ops::DownloadControl {
    let control = ops::DownloadControl::default();
    let worker_control = control.clone();
    if let Some(app) = app_weak.upgrade() {
        match product {
            InstallProduct::Manager => {
                app.set_downloading(true);
                app.set_manager_progress(0.0);
                app.set_manager_progress_text(i18n::tr("install.starting", lang).into());
            }
            InstallProduct::Xiaoai => {
                app.set_xiaoai_busy(true);
                app.set_xiaoai_progress(0.0);
                app.set_xiaoai_progress_text(i18n::tr("install.starting", lang).into());
            }
        }
    }
    std::thread::spawn(move || {
        let progress_weak = app_weak.clone();
        let progress_control = worker_control.clone();
        let mut report = move |progress: ops::DownloadProgress| {
            let weak = progress_weak.clone();
            let control = progress_control.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    let cancelled = control.check_cancelled().is_err();
                    let active = progress.phase != ops::DownloadPhase::Complete && !cancelled;
                    let text = if cancelled {
                        i18n::tr("install.cancelling", lang).into()
                    } else {
                        ops::download_progress_text(progress, lang).into()
                    };
                    match product {
                        InstallProduct::Manager => {
                            app.set_manager_progress(progress.fraction());
                            app.set_manager_progress_text(text);
                            app.set_manager_download_active(active);
                        }
                        InstallProduct::Xiaoai => {
                            app.set_xiaoai_progress(progress.fraction());
                            app.set_xiaoai_progress_text(text);
                            app.set_xiaoai_download_active(active);
                        }
                    }
                }
            });
        };
        let result = operation(&worker_control, &mut report);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = app_weak.upgrade() {
                match product {
                    InstallProduct::Manager => {
                        app.set_downloading(false);
                        app.set_manager_download_active(false);
                    }
                    InstallProduct::Xiaoai => {
                        app.set_xiaoai_busy(false);
                        app.set_xiaoai_download_active(false);
                    }
                }
                append_log(&app, &label, result);
            }
        });
    });
    control
}

#[cfg(windows)]
fn setup_callbacks(app: &AppWindow, lang: i18n::Lang) {
    use install::pc_manager_installer::InstallerKind;

    let app_weak = app.as_weak();
    let manager_download = Rc::new(RefCell::new(None::<ops::DownloadControl>));
    let xiaoai_download = Rc::new(RefCell::new(None::<ops::DownloadControl>));

    app.on_refresh({
        let app_weak = app_weak.clone();
        move || refresh(&app_weak.unwrap())
    });

    app.on_apply_locale({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.locale.apply", lang), || {
            ops::apply_locale(None, "CN", true, false)
        })
    });
    app.on_revert_locale({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.locale.revert", lang), || {
            ops::revert_locale(None, true, false)
        })
    });
    app.on_apply_device({
        let app_weak = app_weak.clone();
        move |model: SharedString| {
            let m = model.to_string();
            let label = i18n::tr("gui.op.device.apply", lang).replace("{model}", &m);
            run_patch(&app_weak.unwrap(), &label, || ops::apply_device(&m, None, false));
        }
    });
    app.on_revert_device({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.device.revert", lang), || {
            ops::revert_device(None, false)
        })
    });
    app.on_apply_camera({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.camera.apply", lang), || {
            ops::apply_camera(None, false)
        })
    });
    app.on_revert_camera({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.camera.revert", lang), || {
            ops::revert_camera(None, false)
        })
    });
    app.on_apply_audio_wifi({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.audio.wifi", lang), || {
            ops::apply_audio(ops::BroadcastMode::Wireless, None, false)
        })
    });
    app.on_apply_audio_lan({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.audio.lan", lang), || {
            ops::apply_audio(ops::BroadcastMode::Wired, None, false)
        })
    });
    app.on_revert_audio({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.audio.revert", lang), || {
            ops::revert_audio(None, false)
        })
    });

    app.on_diagnose_dual_nic({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            let label = i18n::tr("gui.op.dualnic.diagnose", lang);
            match ops::resolve_full_version_dir() {
                Ok(dir) => run_patch(&app, label, || {
                    mipcmanager_patch::experimental::audio_dual_nic::diagnose(&dir)
                }),
                Err(e) => append_log(&app, label, Err(e)),
            }
        }
    });
    app.on_fix_dual_nic({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            let label = i18n::tr("gui.op.dualnic.fix", lang);
            match ops::resolve_full_version_dir() {
                Ok(dir) => run_patch(&app, label, || {
                    mipcmanager_patch::experimental::audio_dual_nic::auto_fix(&dir)
                }),
                Err(e) => append_log(&app, label, Err(e)),
            }
        }
    });

    app.on_apply_smbios({
        let app_weak = app_weak.clone();
        move |model: SharedString| {
            let m = model.to_string();
            let label = i18n::tr("gui.op.smbios.apply", lang).replace("{model}", &m);
            run_patch(&app_weak.unwrap(), &label, || ops::apply_smbios(Some(&m), None, false));
        }
    });
    app.on_revert_smbios({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.smbios.revert", lang), || {
            ops::revert_smbios(None, false)
        })
    });
    app.on_apply_xiaoai({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.xiaoai.apply", lang), || {
            ops::apply_xiaoai(None, false)
        })
    });
    app.on_revert_xiaoai({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.xiaoai.revert", lang), || {
            ops::revert_xiaoai(None, false)
        })
    });

    app.on_clear_log({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_log_text("".into());
            app.set_last_error("".into());
        }
    });
    app.on_uninstall_msix({
        let app_weak = app_weak.clone();
        move || run_patch(&app_weak.unwrap(), i18n::tr("gui.op.uninstall.msix", lang), || {
            ops::uninstall_msix(false)
        })
    });
    app.on_request_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            match ops::uninstall_product_description() {
                Ok(d) => {
                    app.set_confirm_xiaoai(false);
                    app.set_confirm_desc(d.into());
                    app.set_show_confirm(true);
                }
                Err(e) => append_log(&app, i18n::tr("gui.op.uninstall.product", lang), Err(e)),
            }
        }
    });
    app.on_request_xiaoai_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            match xiaoai_uninstall_description() {
                Ok(d) => {
                    app.set_confirm_xiaoai(true);
                    app.set_confirm_desc(d.into());
                    app.set_show_confirm(true);
                }
                Err(e) => append_log(&app, i18n::tr("install.row.xiaoai", lang), Err(e)),
            }
        }
    });
    app.on_confirm_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            let xiaoai = app.get_confirm_xiaoai();
            app.set_show_confirm(false);
            app.set_confirm_xiaoai(false);
            app.set_confirm_desc("".into());
            if xiaoai {
                run_patch(&app, i18n::tr("install.row.xiaoai", lang), uninstall_xiaoai);
            } else {
                run_patch(&app, i18n::tr("gui.op.uninstall.product", lang), ops::uninstall_product);
            }
        }
    });
    app.on_cancel_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_show_confirm(false);
            app.set_confirm_xiaoai(false);
            app.set_confirm_desc("".into());
        }
    });

    app.on_install_recommended({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move || {
            let Some(app) = app_weak.upgrade() else { return; };
            if app.get_downloading() { return; }
            let Some(source) = ops::RecommendedInstaller::MANAGER_VARIANTS
                .get(app.get_manager_source_idx() as usize)
                .copied()
            else { return; };
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                source.label(lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| ops::download_and_install_recommended(source, control, progress),
            ));
        }
    });

    app.on_install_recommended_xiaoai({
        let app_weak = app_weak.clone();
        let download = xiaoai_download.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| app.get_xiaoai_busy()) { return; }
            let source = ops::RecommendedInstaller::Xiaoai;
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                source.label(lang).into(),
                InstallProduct::Xiaoai,
                lang,
                move |control, progress| ops::download_and_install_recommended(source, control, progress),
            ));
        }
    });

    app.on_cancel_download({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move || {
            if let Some(control) = download.borrow().as_ref() { control.cancel(); }
            if let Some(app) = app_weak.upgrade() {
                app.set_manager_download_active(false);
                app.set_manager_progress_text(i18n::tr("install.cancelling", lang).into());
            }
        }
    });
    app.on_cancel_xiaoai_download({
        let app_weak = app_weak.clone();
        let download = xiaoai_download.clone();
        move || {
            if let Some(control) = download.borrow().as_ref() { control.cancel(); }
            if let Some(app) = app_weak.upgrade() {
                app.set_xiaoai_download_active(false);
                app.set_xiaoai_progress_text(i18n::tr("install.cancelling", lang).into());
            }
        }
    });

    app.on_download_and_install_manager({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move |url: SharedString| {
            let url = url.to_string();
            if url.trim().is_empty() { return; }
            let Some(app) = app_weak.upgrade() else { return; };
            if app.get_downloading() { return; }
            if let Err(e) = ensure_manual_url_kind(&url, InstallerKind::XiaomiPcManager) {
                append_log(&app, i18n::tr("install.source.manager", lang), Err(e));
                return;
            }
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                i18n::tr("install.source.manager", lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| ops::download_and_install_pc_manager(Some(&url), control, progress),
            ));
        }
    });

    app.on_browse_manager_installer({
        let app_weak = app_weak.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| app.get_downloading()) { return; }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.source.manager", lang))
                .pick_file()
            {
                app_weak.unwrap().set_manager_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_manager_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let app = app_weak.unwrap();
            if app.get_downloading() { return; }
            let path = PathBuf::from(path.to_string());
            if let Err(e) = ensure_manual_installer_kind(&path, InstallerKind::XiaomiPcManager) {
                append_log(&app, i18n::tr("install.source.manager", lang), Err(e));
                return;
            }
            let label = i18n::tr("gui.op.install", lang)
                .replace("{path}", &path.display().to_string());
            spawn_install_operation(
                app_weak.clone(),
                label,
                InstallProduct::Manager,
                lang,
                move |_, _| ops::install_from_path(&path),
            );
            app.set_manager_path_input("".into());
        }
    });

    app.on_download_and_install_continuity({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move |url: SharedString| {
            let url = url.to_string();
            if url.trim().is_empty() { return; }
            let Some(app) = app_weak.upgrade() else { return; };
            if app.get_downloading() { return; }
            if let Err(e) = ensure_manual_url_kind(&url, InstallerKind::PcContinuity) {
                append_log(&app, i18n::tr("install.kind.continuity", lang), Err(e));
                return;
            }
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                i18n::tr("install.kind.continuity", lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| ops::download_and_install_pc_manager(Some(&url), control, progress),
            ));
        }
    });

    app.on_browse_continuity_installer({
        let app_weak = app_weak.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| app.get_downloading()) { return; }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.kind.continuity", lang))
                .pick_file()
            {
                app_weak.unwrap().set_continuity_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_continuity_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let app = app_weak.unwrap();
            if app.get_downloading() { return; }
            let path = PathBuf::from(path.to_string());
            if let Err(e) = ensure_manual_installer_kind(&path, InstallerKind::PcContinuity) {
                append_log(&app, i18n::tr("install.kind.continuity", lang), Err(e));
                return;
            }
            let label = i18n::tr("gui.op.install", lang)
                .replace("{path}", &path.display().to_string());
            spawn_install_operation(
                app_weak.clone(),
                label,
                InstallProduct::Manager,
                lang,
                move |_, _| ops::install_from_path(&path),
            );
            app.set_continuity_path_input("".into());
        }
    });

    app.on_download_and_install_xiaoai({
        let app_weak = app_weak.clone();
        let download = xiaoai_download.clone();
        move |url: SharedString| {
            let url = url.to_string();
            if url.trim().is_empty() { return; }
            let Some(app) = app_weak.upgrade() else { return; };
            if app.get_xiaoai_busy() { return; }
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                i18n::tr("install.xiaoai.title", lang).to_string(),
                InstallProduct::Xiaoai,
                lang,
                move |control, progress| ops::download_and_install_xiaoai(&url, control, progress),
            ));
        }
    });

    app.on_browse_xiaoai_installer({
        let app_weak = app_weak.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| app.get_xiaoai_busy()) { return; }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.xiaoai.title", lang))
                .pick_file()
            {
                app_weak.unwrap().set_xiaoai_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_xiaoai_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let path = PathBuf::from(path.to_string());
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) { return; }
            let app = app_weak.unwrap();
            if app.get_xiaoai_busy() { return; }
            app.set_xiaoai_path_input("".into());
            let label = i18n::tr("gui.op.xiaoai.install", lang)
                .replace("{path}", &path.display().to_string());
            spawn_install_operation(
                app_weak.clone(),
                label,
                InstallProduct::Xiaoai,
                lang,
                move |_, _| ops::install_xiaoai_from_path(&path),
            );
        }
    });
}
