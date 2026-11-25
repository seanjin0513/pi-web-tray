use std::{
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use tauri::{
    AppHandle, Manager, RunEvent, Wry,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::{
    process::Result,
    runtime::{Package, Runtime},
    service::{Service, URL},
};

#[derive(Clone, Copy)]
enum Action {
    Start,
    Open,
    Restart,
    Stop,
    Check(Package),
    Quit,
}

impl Action {
    fn from_menu(id: &str) -> Option<Self> {
        Some(match id {
            "start" => Self::Start,
            "open" => Self::Open,
            "restart" => Self::Restart,
            "stop" => Self::Stop,
            "check-pi" => Self::Check(Package::Pi),
            "check-web" => Self::Check(Package::Web),
            "quit" => Self::Quit,
            _ => return None,
        })
    }
}

#[derive(Clone)]
struct Items {
    status: MenuItem<Wry>,
    open: MenuItem<Wry>,
    start: MenuItem<Wry>,
    restart: MenuItem<Wry>,
    stop: MenuItem<Wry>,
    pi: MenuItem<Wry>,
    web: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

impl Items {
    fn show(&self, text: &str, busy: bool, service: &Service) {
        let _ = self.status.set_text(format!("Pi Web：{text}"));
        let _ = self.open.set_enabled(!busy && service.is_ready());
        let _ = self.start.set_enabled(!busy && !service.is_started());
        let _ = self.restart.set_enabled(!busy && service.is_started());
        let _ = self.stop.set_enabled(!busy && service.is_started());
        let _ = self.pi.set_enabled(!busy);
        let _ = self.web.set_enabled(!busy);
        let _ = self.quit.set_enabled(!busy);
    }
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
        .plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "macos")]
    let builder = builder.enable_macos_default_menu(false);

    let app = builder
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let (tx, rx) = mpsc::channel::<Action>();
            app.manage(tx.clone());
            let item = |id, text, enabled| MenuItem::with_id(app, id, text, enabled, None::<&str>);
            let items = Items {
                status: item("status", "Pi Web：启动中…", false)?,
                open: item("open", "在浏览器中打开", false)?,
                start: item("start", "启动服务", false)?,
                restart: item("restart", "重启服务", false)?,
                stop: item("stop", "停止服务", false)?,
                pi: item("check-pi", "检查并更新 Pi CLI…", false)?,
                web: item("check-web", "检查并更新 Pi Web…", false)?,
                quit: item("quit", "退出 Pi Web Tray", false)?,
            };
            let service_separator = PredefinedMenuItem::separator(app)?;
            let update_separator = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[
                    &items.status,
                    &items.open,
                    &items.start,
                    &items.restart,
                    &items.stop,
                    &service_separator,
                    &items.pi,
                    &items.web,
                    &update_separator,
                    &items.quit,
                ],
            )?;
            let builder = TrayIconBuilder::with_id("pi-web-tray")
                .menu(&menu)
                .tooltip("Pi Web Tray")
                .show_menu_on_left_click(true)
                .on_menu_event(move |_, event| {
                    if let Some(action) = Action::from_menu(event.id().as_ref()) {
                        let _ = tx.send(action);
                    }
                });
            let builder = builder.icon(
                app.default_window_icon()
                    .expect("packaged application icon")
                    .clone(),
            );
            builder.build(app)?;

            let app = app.handle().clone();
            thread::spawn(move || {
                let mut service = Service::default();
                dispatch(&app, &items, &mut service, Action::Start);
                loop {
                    match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(action) => {
                            if dispatch(&app, &items, &mut service, action) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Err(error) = service.poll() {
                                items.show("服务异常", false, &service);
                                alert(&app, "Pi Web 服务异常", error, MessageDialogKind::Error);
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("启动 Pi Web Tray 失败");

    app.run(|app, event| {
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            let _ = app.state::<Sender<Action>>().send(Action::Quit);
        }
    });
}

fn start(service: &mut Service) -> Result<()> {
    let runtime = Runtime::find()?;
    let web = runtime.installed(Package::Web)?;
    service.start(runtime.command(&web))
}

fn dispatch(app: &AppHandle, items: &Items, service: &mut Service, action: Action) -> bool {
    let phase = match action {
        Action::Start => "启动中…",
        Action::Open => "打开浏览器…",
        Action::Restart => "重启中…",
        Action::Stop | Action::Quit => "停止中…",
        Action::Check(_) => "检查更新中…",
    };
    items.show(phase, true, service);
    let result = match action {
        Action::Start => start(service),
        Action::Open => service.poll().and_then(|_| {
            if !service.is_ready() {
                return Err("服务还没有启动。".into());
            }
            open::that(URL).map_err(|e| format!("无法打开默认浏览器：{e}"))
        }),
        Action::Restart => service.stop().and_then(|_| start(service)),
        Action::Stop | Action::Quit => service.stop(),
        Action::Check(package) => check(app, items, service, package),
    };
    let status = if service.is_ready() {
        "运行中 · 127.0.0.1:30141"
    } else if result.is_err() {
        "操作失败"
    } else {
        "已停止"
    };
    items.show(status, false, service);
    match result {
        Ok(()) if matches!(action, Action::Quit) => {
            app.exit(0);
            return true;
        }
        Err(error) => alert(app, "Pi Web Tray", error, MessageDialogKind::Error),
        Ok(()) => {}
    }
    false
}

fn check(app: &AppHandle, items: &Items, service: &mut Service, package: Package) -> Result<()> {
    let runtime = Runtime::find()?;
    let previous = runtime.installed(package)?;
    let latest = runtime.latest(package)?;
    let label = package.label();
    let internal = previous
        .internal_pi
        .as_ref()
        .filter(|_| package == Package::Web)
        .map(|v| format!("\n当前网页使用的 Pi 依赖：{v}"))
        .unwrap_or_default();
    let versions = format!(
        "当前版本：{}\nnpm 稳定版：{latest}{internal}",
        previous.version
    );
    if latest <= previous.version {
        alert(app, label, versions, MessageDialogKind::Info);
        return Ok(());
    }
    let restart = package == Package::Web && service.is_started();
    let note = match package {
        Package::Web if restart => {
            "服务将暂停后重启，运行中的任务可能中断。网页使用的 Pi 随 Pi Web 更新。"
        }
        Package::Web => "网页使用的 Pi 随 Pi Web 更新。已停止的服务不会自动启动。",
        Package::Pi => "仅更新全局 Pi CLI，不会更新或重启 Pi Web。",
    };
    if !app
        .dialog()
        .message(format!("{versions}\n\n{note}\n\n现在更新 {label}？"))
        .title("确认更新")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "更新".into(),
            "取消".into(),
        ))
        .blocking_show()
    {
        return Ok(());
    }
    items.show("更新中…", true, service);
    if restart {
        service.stop()?;
    }
    let install = (|| {
        if package == Package::Web {
            runtime.ensure_web_stopped(&previous)?;
        }
        runtime.install(package, &latest)
    })();
    let restore = if restart {
        items.show("重新启动服务…", true, service);
        start(service)
    } else {
        Ok(())
    };
    match (install, restore) {
        (Ok(updated), Ok(())) => {
            let internal = updated
                .internal_pi
                .filter(|_| package == Package::Web)
                .map(|v| format!("\n网页使用的 Pi 依赖：{v}"))
                .unwrap_or_default();
            alert(
                app,
                "更新完成",
                format!("{label} 已更新到 {}。{internal}", updated.version),
                MessageDialogKind::Info,
            );
            Ok(())
        }
        (Ok(updated), Err(error)) => Err(format!(
            "{label} 已更新到 {}，但服务未能重启。\n{error}",
            updated.version
        )),
        (Err(error), Ok(())) if restart => {
            Err(format!("更新未完成，但 Pi Web 服务已恢复。\n{error}"))
        }
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restart)) => Err(format!(
            "更新未完成，服务也未能恢复。\n{error}\n\n{restart}"
        )),
    }
}

fn alert(app: &AppHandle, title: &str, message: String, kind: MessageDialogKind) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(kind)
        .show(|_| {});
}
