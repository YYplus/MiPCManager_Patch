#![windows_subsystem = "windows"]

use anyhow::{Result, bail};
use mipcmanager_patch::{elevate, i18n, install, ops, patches::device as ds};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
#[cfg(windows)]
use std::cell::RefCell;
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
        if key == "install.one-click" {
            return match lang {
                i18n::Lang::Zh => "一键安装".into(),
                i18n::Lang::En => "One-click Install".into(),
            };
        }
        i18n::tr(&key, lang).into()
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

fn refresh(app: &AppWindow) {
    app.set_full_features(ops::full_features_available());
    app.set_continuity_available(install::find_pc_continuity_root().is_some());
    app.set_xiaoai_available(ops::xiaoai_available());
    app.set_status_text(ops::status_lines().join("\n").into());
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
                    app.set_confirm_desc(d.into());
                    app.set_show_confirm(true);
                }
                Err(e) => append_log(&app, i18n::tr("gui.op.uninstall.product", lang), Err(e)),
            }
        }
    });
    app.on_confirm_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_show_confirm(false);
            app.set_confirm_desc("".into());
            run_patch(&app, i18n::tr("gui.op.uninstall.product", lang), ops::uninstall_product);
        }
    });
    app.on_cancel_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_show_confirm(false);
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
