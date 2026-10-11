//! RCWLP host as a tray app: the same server as the console host, with an icon in the
//! notification area, a log window, a pairing dialog and "Start with Windows".

#![windows_subsystem = "windows"]

mod theme;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use native_windows_gui as nwg;
use rcwlp_host::autostart;
use rcwlp_host::platform::{self, Config};
use rcwlp_host::protocol;
use rcwlp_host::server::{self, Out};

/// Everything the server said since the window last looked, plus how the server ended.
#[derive(Default)]
struct Inbox {
    out: Vec<Out>,
    finished: Option<Result<(), platform::StartError>>,
}

struct Tray {
    owner: nwg::MessageWindow,
    /// The icon at the small size (tray, title bars) and the large one (notifications).
    icon: nwg::Icon,
    icon_big: nwg::Icon,
    tray: nwg::TrayNotification,
    menu: nwg::Menu,
    menu_show: nwg::MenuItem,
    menu_autostart: nwg::MenuItem,
    menu_quit: nwg::MenuItem,
    notice: nwg::Notice,
    fonts: theme::Fonts,
    /// The custom painting bound to the windows and buttons below.
    _looks: Vec<nwg::RawEventHandler>,

    log_window: nwg::Window,
    log_title: nwg::Label,
    log: nwg::TextBox,
    command: nwg::TextInput,
    run: nwg::Button,

    pair_window: nwg::Window,
    pair_title: nwg::Label,
    pair_label: nwg::Label,
    pair_code: nwg::TextInput,
    pair_accept: nwg::Button,
    pair_reject: nwg::Button,

    commands: Sender<String>,
    inbox: Arc<Mutex<Inbox>>,
    server: RefCell<Option<JoinHandle<()>>>,
}

fn main() {
    nwg::init().expect("start the Windows UI");
    let _ = nwg::Font::set_global_family("Segoe UI");
    let verbose = std::env::args().any(|a| a == "--verbose" || a == "-v");
    let port = port_arg().unwrap_or(protocol::DEFAULT_PORT);
    // Starting it from the Start Menu while it already runs at sign-in shouldn't start a second one.
    let Some(_instance) = SingleInstance::claim(port) else {
        nwg::message(&nwg::MessageParams {
            title: "RCWLP host",
            content: "RCWLP host is already running. Its icon is in the notification area, next to the clock \
                      (it may be behind the ^ arrow).",
            buttons: nwg::MessageButtons::Ok,
            icons: nwg::MessageIcons::Info,
        });
        return;
    };

    let (commands, command_rx) = mpsc::channel();
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    let tray = Rc::new(build(commands, inbox.clone()).expect("build the tray UI"));

    let sender = tray.notice.sender();
    let out_inbox = inbox.clone();
    let out = Box::new(move |o: Out| {
        out_inbox.lock().unwrap().out.push(o);
        sender.notice();
    });
    let done_sender = tray.notice.sender();
    let done_inbox = inbox;
    let server = thread::spawn(move || {
        let result = platform::run(Config { port, verbose }, out, command_rx);
        done_inbox.lock().unwrap().finished = Some(result);
        done_sender.notice();
    });
    *tray.server.borrow_mut() = Some(server);

    let handlers: Vec<_> = [tray.owner.handle, tray.log_window.handle, tray.pair_window.handle]
        .into_iter()
        .map(|window| {
            let tray = Rc::downgrade(&tray);
            nwg::full_bind_event_handler(&window, move |event, data, handle| {
                if let Some(tray) = tray.upgrade() {
                    tray.on_event(event, &data, handle);
                }
            })
        })
        .collect();
    nwg::dispatch_thread_events();
    handlers.iter().for_each(nwg::unbind_event_handler);
}

/// A named mutex held while this tray app runs, so a second copy on the same port can tell.
struct SingleInstance(winapi::shared::ntdef::HANDLE);

impl SingleInstance {
    fn claim(port: u16) -> Option<SingleInstance> {
        use winapi::shared::winerror::ERROR_ALREADY_EXISTS;
        use winapi::um::errhandlingapi::GetLastError;
        use winapi::um::synchapi::CreateMutexW;
        // Hosts on different ports don't get in each other's way.
        let name = match port {
            protocol::DEFAULT_PORT => "Local\\RCWLP-host-tray".to_string(),
            _ => format!("Local\\RCWLP-host-tray-{port}"),
        };
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // Safety: plain Win32 calls; the handle is closed in Drop.
        unsafe {
            let handle = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
            if handle.is_null() {
                return Some(SingleInstance(handle)); // Can't tell; let the port check catch a duplicate.
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                winapi::um::handleapi::CloseHandle(handle);
                return None;
            }
            Some(SingleInstance(handle))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Safety: the handle came from CreateMutexW.
            unsafe { winapi::um::handleapi::CloseHandle(self.0) };
        }
    }
}

/// Opens a web page in the default browser.
fn open_url(url: &str) {
    use winapi::um::shellapi::ShellExecuteW;
    use winapi::um::winuser::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (open, url) = (wide("open"), wide(url));
    // Safety: plain Win32 call with NUL-terminated strings.
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn port_arg() -> Option<u16> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--port")?;
    args.get(i + 1)?.parse().ok()
}

fn build(commands: Sender<String>, inbox: Arc<Mutex<Inbox>>) -> Result<Tray, nwg::NwgError> {
    let mut t = Tray {
        owner: Default::default(),
        icon: theme::app_icon(false),
        icon_big: theme::app_icon(true),
        tray: Default::default(),
        menu: Default::default(),
        menu_show: Default::default(),
        menu_autostart: Default::default(),
        menu_quit: Default::default(),
        notice: Default::default(),
        // Before any control is made, so they all get Montserrat.
        fonts: theme::Fonts::load(),
        _looks: Vec::new(),
        log_window: Default::default(),
        log_title: Default::default(),
        log: Default::default(),
        command: Default::default(),
        run: Default::default(),
        pair_window: Default::default(),
        pair_title: Default::default(),
        pair_label: Default::default(),
        pair_code: Default::default(),
        pair_accept: Default::default(),
        pair_reject: Default::default(),
        commands,
        inbox,
        server: RefCell::new(None),
    };

    nwg::MessageWindow::builder().build(&mut t.owner)?;
    nwg::TrayNotification::builder()
        .parent(&t.owner)
        .icon(Some(&t.icon))
        .tip(Some("RCWLP host: starting"))
        .build(&mut t.tray)?;
    nwg::Menu::builder().popup(true).parent(&t.owner).build(&mut t.menu)?;
    nwg::MenuItem::builder().text("Show log").parent(&t.menu).build(&mut t.menu_show)?;
    nwg::MenuItem::builder()
        .text("Start with Windows")
        .check(autostart::RCWLP.get().is_some())
        .parent(&t.menu)
        .build(&mut t.menu_autostart)?;
    nwg::MenuItem::builder().text("Quit").parent(&t.menu).build(&mut t.menu_quit)?;
    nwg::Notice::builder().parent(&t.owner).build(&mut t.notice)?;

    // Text fields sit inside the rounded frames their window draws around them (theme::FIELD_PAD).
    let (w, h) = (680, 460);
    nwg::Window::builder()
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::MINIMIZE_BOX)
        .size((w, h))
        .position(centered(w, h))
        .title("RCWLP host")
        .icon(Some(&t.icon))
        .build(&mut t.log_window)?;
    heading(&t.log_window, "Host log", w, &t.fonts, &mut t.log_title)?;
    nwg::TextBox::builder()
        .parent(&t.log_window)
        .position((34, 54))
        .size((w - 68, h - 140))
        .readonly(true)
        .font(Some(&t.fonts.mono))
        .flags(nwg::TextBoxFlags::VISIBLE | nwg::TextBoxFlags::VSCROLL | nwg::TextBoxFlags::AUTOVSCROLL)
        .build(&mut t.log)?;
    nwg::TextInput::builder()
        .parent(&t.log_window)
        .position((34, h - 52))
        .size((w - 68 - 116, 30))
        .background_color(Some(theme::FIELD))
        .placeholder_text(Some("list, forget <n>, or a pairing code"))
        .build(&mut t.command)?;
    nwg::Button::builder()
        .parent(&t.log_window)
        .text("Run")
        .position((w - 24 - 96, h - 59))
        .size((96, 44))
        .build(&mut t.run)?;

    let (w, h) = (420, 236);
    nwg::Window::builder()
        .flags(nwg::WindowFlags::WINDOW)
        .size((w, h))
        .position(centered(w, h))
        .title("Pair a phone")
        .icon(Some(&t.icon))
        .topmost(true)
        .build(&mut t.pair_window)?;
    heading(&t.pair_window, "Pairing request", w, &t.fonts, &mut t.pair_title)?;
    nwg::Label::builder()
        .parent(&t.pair_window)
        .text("")
        .position((24, 46))
        .size((w - 48, 44))
        .v_align(nwg::VTextAlign::Top)
        .background_color(Some(theme::PAGE))
        .build(&mut t.pair_label)?;
    nwg::TextInput::builder()
        .parent(&t.pair_window)
        .position((34, 106))
        .size((170, 34))
        .limit(7)
        .font(Some(&t.fonts.code))
        .background_color(Some(theme::FIELD))
        .placeholder_text(Some("123 456"))
        .build(&mut t.pair_code)?;
    let button = |text, x, b: &mut nwg::Button| {
        nwg::Button::builder().parent(&t.pair_window).text(text).position((x, h - 60)).size((100, 40)).build(b)
    };
    button("Accept", w - 24 - 100 - 10 - 100, &mut t.pair_accept)?;
    button("Reject", w - 24 - 100, &mut t.pair_reject)?;

    let mut looks = theme::style_window(&t.log_window, &[&t.log.handle, &t.command.handle], &[(&t.log_title, theme::ROSE)]);
    looks.extend(theme::style_window(
        &t.pair_window,
        &[&t.pair_code.handle],
        &[(&t.pair_title, theme::ROSE), (&t.pair_label, theme::TEXT)],
    ));
    looks.push(theme::style_button(&t.run, true, &t.fonts.button));
    looks.push(theme::style_button(&t.pair_accept, true, &t.fonts.button));
    looks.push(theme::style_button(&t.pair_reject, false, &t.fonts.button));
    t._looks = looks;
    Ok(t)
}

/// A window's title inside it, small, heavy and in capitals, as SimonSays heads its panels.
fn heading(window: &nwg::Window, text: &str, w: i32, fonts: &theme::Fonts, label: &mut nwg::Label) -> Result<(), nwg::NwgError> {
    nwg::Label::builder()
        .parent(window)
        .text(&text.to_uppercase())
        .position((24, 18))
        .size((w - 48, 20))
        .font(Some(&fonts.heading))
        .background_color(Some(theme::PAGE))
        .build(label)
}

fn centered(w: i32, h: i32) -> (i32, i32) {
    ((nwg::Monitor::width() - w) / 2, (nwg::Monitor::height() - h) / 2)
}

impl Tray {
    fn on_event(&self, event: nwg::Event, data: &nwg::EventData, handle: nwg::ControlHandle) {
        use nwg::Event as E;
        match event {
            E::OnNotice if handle == self.notice => self.drain(),
            E::OnContextMenu if handle == self.tray => {
                let (x, y) = nwg::GlobalCursor::position();
                self.menu.popup(x, y);
            }
            E::OnMousePress(nwg::MousePressEvent::MousePressLeftUp) if handle == self.tray => self.show_log(),
            E::OnMenuItemSelected if handle == self.menu_show => self.show_log(),
            E::OnMenuItemSelected if handle == self.menu_autostart => self.toggle_autostart(),
            E::OnMenuItemSelected if handle == self.menu_quit => self.quit(),
            E::OnButtonClick if handle == self.run => self.run_command(),
            E::OnKeyEnter if handle == self.command => self.run_command(),
            E::OnButtonClick if handle == self.pair_accept => self.accept(),
            E::OnKeyEnter if handle == self.pair_code => self.accept(),
            E::OnButtonClick if handle == self.pair_reject => {
                let _ = self.commands.send("n".into());
            }
            // Closing a window only hides it; the host keeps running in the tray.
            E::OnWindowClose if handle == self.log_window || handle == self.pair_window => {
                if let nwg::EventData::OnWindowClose(close) = data {
                    close.close(false);
                }
                if handle == self.log_window {
                    self.log_window.set_visible(false);
                } else {
                    // Closing the pairing dialog rejects, as the console's n does.
                    let _ = self.commands.send("n".into());
                }
            }
            _ => {}
        }
    }

    fn drain(&self) {
        let (out, finished) = {
            let mut inbox = self.inbox.lock().unwrap();
            (std::mem::take(&mut inbox.out), inbox.finished.take())
        };
        for o in out {
            match o {
                Out::Event(s) => self.log.appendln(&format!("{} {s}", rcwlp_host::clock::stamp())),
                Out::Debug(s) => self.log.appendln(&format!("{} [verbose] {s}", rcwlp_host::clock::stamp())),
                Out::Reply(s) => {
                    self.log.appendln(&s);
                    // A wrong code typed in the pairing dialog is answered here.
                    if self.pair_window.visible() {
                        self.pair_label.set_text(&s);
                    }
                }
                Out::Pairing(Some(name)) => self.ask_for_code(&name),
                Out::Pairing(None) => self.pair_window.set_visible(false),
                Out::Connected(list) => self.show_connected(&list),
            }
        }
        if let Some(result) = finished {
            match result {
                Ok(()) => {}
                Err(e @ platform::StartError::NoDriver(_)) => {
                    let choice = nwg::message(&nwg::MessageParams {
                        title: "RCWLP host",
                        content: &format!("{e}\n\nOpen the ViGEmBus download page now?"),
                        buttons: nwg::MessageButtons::YesNo,
                        icons: nwg::MessageIcons::Warning,
                    });
                    if choice == nwg::MessageChoice::Yes {
                        open_url(platform::DRIVER_URL);
                    }
                }
                Err(e) => {
                    nwg::modal_error_message(&self.owner, "RCWLP host", &e.to_string());
                }
            }
            self.tray.set_visibility(false);
            nwg::stop_thread_dispatch();
        }
    }

    fn ask_for_code(&self, name: &str) {
        self.pair_label.set_text(&format!(
            "\"{name}\" wants to pair. Type the 6-digit code shown on the phone to accept it."
        ));
        self.pair_code.set_text("");
        self.pair_window.set_visible(true);
        self.pair_code.set_focus();
        self.tray.show(
            &format!("\"{name}\" wants to pair. Type the code shown on the phone."),
            Some("RCWLP pairing request"),
            Some(nwg::TrayNotificationFlags::USER_ICON | nwg::TrayNotificationFlags::LARGE_ICON),
            Some(&self.icon_big),
        );
    }

    fn accept(&self) {
        let code = self.pair_code.text();
        if server::parse_code(&code).is_none() {
            self.pair_label.set_text("Type the 6 digits the phone shows, like 123 456.");
            return;
        }
        let _ = self.commands.send(code);
    }

    fn show_connected(&self, list: &[String]) {
        let tip = match list.len() {
            0 => "RCWLP host: no phone connected".to_string(),
            1 => format!("RCWLP host: {}", list[0]),
            n => format!("RCWLP host: {n} controllers connected"),
        };
        // Tooltips are limited to 127 characters.
        self.tray.set_tip(&tip.chars().take(127).collect::<String>());
    }

    fn show_log(&self) {
        self.log_window.set_visible(true);
        self.log_window.set_focus();
    }

    fn run_command(&self) {
        let line = self.command.text();
        if line.trim().is_empty() {
            return;
        }
        self.log.appendln(&format!("> {line}"));
        self.command.set_text("");
        if line.trim() == "quit" || line.trim() == "exit" {
            return self.quit();
        }
        let _ = self.commands.send(line);
    }

    fn toggle_autostart(&self) {
        let on = autostart::RCWLP.get().is_some();
        let result = if on {
            autostart::RCWLP.remove()
        } else {
            std::env::current_exe().and_then(|exe| autostart::RCWLP.set(&autostart::command_for(&exe)))
        };
        match result {
            Ok(()) => self.menu_autostart.set_checked(!on),
            Err(e) => {
                nwg::modal_error_message(&self.owner, "RCWLP host", &format!("Couldn't change Start with Windows: {e}"));
            }
        }
    }

    fn quit(&self) {
        let _ = self.commands.send("quit".into());
        // The server unplugs its virtual controllers on the way out.
        if let Some(server) = self.server.borrow_mut().take() {
            let _ = server.join();
        }
        self.tray.set_visibility(false);
        nwg::stop_thread_dispatch();
    }
}
