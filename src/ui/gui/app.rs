#![windows_subsystem = "windows"]

use anyhow::Result;
use mipcmanager_patch::{
    elevate,
    experimental::smbios_spoof,
    i18n, install, ops,
    patches::{ai, audio, camera, device as ds, locale},
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

    app.on_tr(move |key: SharedString| -> SharedString { i18n::tr(key.as_str(), lang).into() });

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
    let timer = slint::Timer::default();
    let weak = app.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(5),
        move || {
            if let Some(app) = weak.upgrade() {
                refresh(&app);
            }
        },
    );
    app.run().unwrap();
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

fn sync_device_model(app: &AppWindow, model: &str) {
    if let Some(index) = ds::PRESETS.iter().position(|preset| preset.code == model) {
        app.set_custom_mode(false);
        app.set_model_idx(index as i32);
    } else {
        app.set_custom_mode(true);
        app.set_custom_model_input(model.into());
    }
}

#[derive(Default)]
struct PatchState {
    full: bool,
    continuity: bool,
    xiaoai: bool,
    locale: bool,
    device: bool,
    device_model: Option<String>,
    camera: bool,
    audio: bool,
    audio_mode: i32,
    dual_nic: i32,
    smbios: bool,
    xiaoai_patch: bool,
    share_menu: i32,
}

impl PatchState {
    fn read() -> Self {
        let mut state = Self {
            full: ops::full_features_available(),
            continuity: install::find_pc_continuity_root().is_some(),
            xiaoai: ops::xiaoai_available(),
            locale: ops::resolve_locale_dll(None)
                .ok()
                .as_deref()
                .is_some_and(locale::is_patched),
            ..Self::default()
        };
        if state.full
            && let Ok(version) = ops::resolve_full_version_dir()
        {
            let (proxy, model) = ds::current_state(&version);
            state.device = proxy || model.is_some();
            state.device_model = model;
            state.camera = camera::is_patched(&version.join(camera::TARGET_DLL));
            state.audio_mode = audio_mode(&version);
            let saved_route = audio::wifi_route_state(&version);
            state.audio = [audio::TARGET_MIPCAUDIO, audio::TARGET_IDMRUNTIME]
                .iter()
                .any(|name| differs_from_backup(&version.join(name)))
                || (!saved_route.contains("未配置") && !saved_route.contains("状态不可读"));
            state.dual_nic =
                match mipcmanager_patch::experimental::audio_dual_nic::repair_needed(&version) {
                    Ok(false) => 1,
                    Ok(true) => 2,
                    Err(_) => 3,
                };
            state.smbios = smbios_spoof::is_patched(&version.join(smbios_spoof::TARGET_DLL));
        }
        state.xiaoai_patch = install::find_xiaoai_root()
            .and_then(|root| install::latest_version_dir(&root).ok())
            .is_some_and(|version| ai::current_state(&version));
        state.share_menu = match ops::share_menu_state() {
            ops::ShellMenuState::Disabled => 0,
            ops::ShellMenuState::Enabled => 1,
            ops::ShellMenuState::Partial => 2,
        };
        state
    }

    fn update(self, app: &AppWindow) {
        app.set_full_features(self.full);
        app.set_continuity_available(self.continuity);
        app.set_xiaoai_available(self.xiaoai);
        app.set_locale_active(self.locale);
        app.set_device_active(self.device);
        if let Some(model) = self.device_model {
            sync_device_model(app, &model);
        }
        app.set_camera_active(self.camera);
        app.set_audio_active(self.audio);
        app.set_audio_mode(self.audio_mode);
        app.set_dual_nic_state(self.dual_nic);
        app.set_smbios_active(self.smbios);
        app.set_xiaoai_patch_active(self.xiaoai_patch);
        app.set_share_menu_state(self.share_menu);
        app.set_state_ready(true);
    }
}

fn refresh(app: &AppWindow) {
    if app.get_state_refreshing() || app.get_operation_busy() {
        return;
    }
    app.set_state_refreshing(true);
    let generation = app.get_state_generation();
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let state = PatchState::read();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.set_state_refreshing(false);
                if generation == app.get_state_generation() && !app.get_operation_busy() {
                    state.update(&app);
                } else {
                    refresh(&app);
                }
            }
        });
    });
}

fn operation_allowed(app: &AppWindow) -> bool {
    app.get_state_ready()
        && !app.get_operation_busy()
        && !app.get_downloading()
        && !app.get_xiaoai_busy()
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

fn run_patch(
    app: &AppWindow,
    label: &str,
    f: impl FnOnce() -> Result<Vec<String>> + Send + 'static,
) {
    if !operation_allowed(app) {
        return;
    }
    app.set_operation_busy(true);
    app.set_state_generation(app.get_state_generation() + 1);
    let weak = app.as_weak();
    let label = label.to_string();
    std::thread::spawn(move || {
        let result = f();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.set_operation_busy(false);
                app.set_share_menu_busy(false);
                append_log(&app, &label, result);
            }
        });
    });
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
        app.set_state_generation(app.get_state_generation() + 1);
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
                app.set_state_generation(app.get_state_generation() + 1);
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

    app.on_apply_xiaomi_share_menu({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            if !operation_allowed(&app) {
                return;
            }
            app.set_share_menu_busy(true);
            run_patch(
                &app,
                i18n::tr("gui.op.share-menu.apply", lang),
                ops::apply_share_menu,
            );
        }
    });
    app.on_revert_xiaomi_share_menu({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            if !operation_allowed(&app) {
                return;
            }
            app.set_share_menu_busy(true);
            run_patch(
                &app,
                i18n::tr("gui.op.share-menu.revert", lang),
                ops::revert_share_menu,
            );
        }
    });

    app.on_apply_locale({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.locale.apply", lang),
                || ops::apply_locale(None, "CN", true, false),
            )
        }
    });
    app.on_revert_locale({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.locale.revert", lang),
                || ops::revert_locale(None, true, false),
            )
        }
    });
    app.on_apply_device({
        let app_weak = app_weak.clone();
        move |model: SharedString| {
            let m = model.to_string();
            let label = i18n::tr("gui.op.device.apply", lang).replace("{model}", &m);
            run_patch(&app_weak.unwrap(), &label, move || {
                ops::apply_device(&m, None, false)
            });
        }
    });
    app.on_revert_device({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.device.revert", lang),
                || ops::revert_device(None, false),
            )
        }
    });
    app.on_apply_camera({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.camera.apply", lang),
                || ops::apply_camera(None, false),
            )
        }
    });
    app.on_revert_camera({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.camera.revert", lang),
                || ops::revert_camera(None, false),
            )
        }
    });
    app.on_apply_audio_wifi({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.audio.wifi", lang),
                || ops::apply_audio(ops::BroadcastMode::Wireless, None, false),
            )
        }
    });
    app.on_apply_audio_lan({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.audio.lan", lang),
                || ops::apply_audio(ops::BroadcastMode::Wired, None, false),
            )
        }
    });
    app.on_revert_audio({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.audio.revert", lang),
                || ops::revert_audio(None, false),
            )
        }
    });

    app.on_diagnose_dual_nic({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            let label = i18n::tr("gui.op.dualnic.diagnose", lang);
            match ops::resolve_full_version_dir() {
                Ok(dir) => run_patch(&app, label, move || {
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
                Ok(dir) => run_patch(&app, label, move || {
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
            run_patch(&app_weak.unwrap(), &label, move || {
                ops::apply_smbios(Some(&m), None, false)
            });
        }
    });
    app.on_revert_smbios({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.smbios.revert", lang),
                || ops::revert_smbios(None, false),
            )
        }
    });
    app.on_apply_xiaoai({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.xiaoai.apply", lang),
                || ops::apply_xiaoai(None, false),
            )
        }
    });
    app.on_revert_xiaoai({
        let app_weak = app_weak.clone();
        move || {
            run_patch(
                &app_weak.unwrap(),
                i18n::tr("gui.op.xiaoai.revert", lang),
                || ops::revert_xiaoai(None, false),
            )
        }
    });

    app.on_clear_log({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_log_text("".into());
            app.set_last_error("".into());
        }
    });
    app.on_request_uninstall({
        let app_weak = app_weak.clone();
        move |kind: i32| {
            let app = app_weak.unwrap();
            let product = if kind == 0 {
                ops::SoftwareProduct::PcManager
            } else {
                ops::SoftwareProduct::Continuity
            };
            match ops::uninstall_software_description(product) {
                Ok(d) => {
                    app.set_confirm_product(kind);
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
            match ops::uninstall_software_description(ops::SoftwareProduct::Xiaoai) {
                Ok(d) => {
                    app.set_confirm_product(2);
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
            let product = match app.get_confirm_product() {
                0 => ops::SoftwareProduct::PcManager,
                1 => ops::SoftwareProduct::Continuity,
                _ => ops::SoftwareProduct::Xiaoai,
            };
            app.set_show_confirm(false);
            app.set_confirm_product(0);
            app.set_confirm_desc("".into());
            run_patch(
                &app,
                i18n::tr("gui.op.uninstall.product", lang),
                move || ops::uninstall_software(product),
            );
        }
    });
    app.on_cancel_uninstall({
        let app_weak = app_weak.clone();
        move || {
            let app = app_weak.unwrap();
            app.set_show_confirm(false);
            app.set_confirm_product(0);
            app.set_confirm_desc("".into());
        }
    });

    app.on_install_recommended({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !operation_allowed(&app) || app.get_full_features() || app.get_continuity_available()
            {
                return;
            }
            let Some(source) = ops::RecommendedInstaller::MANAGER_VARIANTS
                .get(app.get_manager_source_idx() as usize)
                .copied()
            else {
                return;
            };
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                source.label(lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| {
                    let kind = if source == ops::RecommendedInstaller::PcManager {
                        InstallerKind::XiaomiPcManager
                    } else {
                        InstallerKind::PcContinuity
                    };
                    ops::download_and_install_product(source.url(), kind, control, progress)
                },
            ));
        }
    });

    app.on_install_recommended_xiaoai({
        let app_weak = app_weak.clone();
        let download = xiaoai_download.clone();
        move || {
            if app_weak
                .upgrade()
                .is_none_or(|app| !operation_allowed(&app) || app.get_xiaoai_available())
            {
                return;
            }
            let source = ops::RecommendedInstaller::Xiaoai;
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                source.label(lang).into(),
                InstallProduct::Xiaoai,
                lang,
                move |control, progress| {
                    ops::download_and_install_recommended(source, control, progress)
                },
            ));
        }
    });

    app.on_cancel_download({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move || {
            if let Some(control) = download.borrow().as_ref() {
                control.cancel();
            }
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
            if let Some(control) = download.borrow().as_ref() {
                control.cancel();
            }
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
            if url.trim().is_empty() {
                return;
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !operation_allowed(&app) || app.get_full_features() || app.get_continuity_available()
            {
                return;
            }
            if let Err(e) = ops::ensure_manual_url_kind(&url, InstallerKind::XiaomiPcManager) {
                append_log(&app, i18n::tr("install.source.manager", lang), Err(e));
                return;
            }
            app.set_manager_source_idx(0);
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                i18n::tr("install.source.manager", lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| {
                    ops::download_and_install_product(
                        &url,
                        InstallerKind::XiaomiPcManager,
                        control,
                        progress,
                    )
                },
            ));
        }
    });

    app.on_browse_manager_installer({
        let app_weak = app_weak.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| {
                !operation_allowed(&app)
                    || app.get_full_features()
                    || app.get_continuity_available()
            }) {
                return;
            }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.source.manager", lang))
                .pick_file()
            {
                app_weak
                    .unwrap()
                    .set_manager_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_manager_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let app = app_weak.unwrap();
            if !operation_allowed(&app) || app.get_full_features() || app.get_continuity_available()
            {
                return;
            }
            let path = PathBuf::from(path.to_string());
            if let Err(e) = ops::ensure_manual_installer_kind(&path, InstallerKind::XiaomiPcManager)
            {
                append_log(&app, i18n::tr("install.source.manager", lang), Err(e));
                return;
            }
            let label =
                i18n::tr("gui.op.install", lang).replace("{path}", &path.display().to_string());
            app.set_manager_source_idx(0);
            spawn_install_operation(
                app_weak.clone(),
                label,
                InstallProduct::Manager,
                lang,
                move |_, _| ops::install_product_from_path(&path, InstallerKind::XiaomiPcManager),
            );
            app.set_manager_path_input("".into());
        }
    });

    app.on_download_and_install_continuity({
        let app_weak = app_weak.clone();
        let download = manager_download.clone();
        move |url: SharedString| {
            let url = url.to_string();
            if url.trim().is_empty() {
                return;
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !operation_allowed(&app) || app.get_full_features() || app.get_continuity_available()
            {
                return;
            }
            if let Err(e) = ops::ensure_manual_url_kind(&url, InstallerKind::PcContinuity) {
                append_log(&app, i18n::tr("install.kind.continuity", lang), Err(e));
                return;
            }
            app.set_manager_source_idx(app.get_continuity_source_idx() + 1);
            *download.borrow_mut() = Some(spawn_install_operation(
                app_weak.clone(),
                i18n::tr("install.kind.continuity", lang).into(),
                InstallProduct::Manager,
                lang,
                move |control, progress| {
                    ops::download_and_install_product(
                        &url,
                        InstallerKind::PcContinuity,
                        control,
                        progress,
                    )
                },
            ));
        }
    });

    app.on_browse_continuity_installer({
        let app_weak = app_weak.clone();
        move || {
            if app_weak.upgrade().is_none_or(|app| {
                !operation_allowed(&app)
                    || app.get_full_features()
                    || app.get_continuity_available()
            }) {
                return;
            }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.kind.continuity", lang))
                .pick_file()
            {
                app_weak
                    .unwrap()
                    .set_continuity_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_continuity_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let app = app_weak.unwrap();
            if !operation_allowed(&app) || app.get_full_features() || app.get_continuity_available()
            {
                return;
            }
            let path = PathBuf::from(path.to_string());
            if let Err(e) = ops::ensure_manual_installer_kind(&path, InstallerKind::PcContinuity) {
                append_log(&app, i18n::tr("install.kind.continuity", lang), Err(e));
                return;
            }
            let label =
                i18n::tr("gui.op.install", lang).replace("{path}", &path.display().to_string());
            app.set_manager_source_idx(app.get_continuity_source_idx() + 1);
            spawn_install_operation(
                app_weak.clone(),
                label,
                InstallProduct::Manager,
                lang,
                move |_, _| ops::install_product_from_path(&path, InstallerKind::PcContinuity),
            );
            app.set_continuity_path_input("".into());
        }
    });

    app.on_download_and_install_xiaoai({
        let app_weak = app_weak.clone();
        let download = xiaoai_download.clone();
        move |url: SharedString| {
            let url = url.to_string();
            if url.trim().is_empty() {
                return;
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !operation_allowed(&app) || app.get_xiaoai_available() {
                return;
            }
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
            if app_weak
                .upgrade()
                .is_none_or(|app| !operation_allowed(&app) || app.get_xiaoai_available())
            {
                return;
            }
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(i18n::tr("gui.browse.filter", lang), &["exe"])
                .set_title(i18n::tr("install.xiaoai.title", lang))
                .pick_file()
            {
                app_weak
                    .unwrap()
                    .set_xiaoai_path_input(path.display().to_string().into());
            }
        }
    });

    app.on_start_xiaoai_install({
        let app_weak = app_weak.clone();
        move |path: SharedString| {
            let path = PathBuf::from(path.to_string());
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            {
                return;
            }
            let app = app_weak.unwrap();
            if !operation_allowed(&app) || app.get_xiaoai_available() {
                return;
            }
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
