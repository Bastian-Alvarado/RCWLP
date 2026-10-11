//! Gives the programs their icon and version details in Explorer, the taskbar and the
//! installer's shortcuts.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rcwlp.ico")
            .set("FileDescription", "RCWLP host: phones as game controllers")
            .set("ProductName", "RCWLP");
        res.compile().expect("embed the Windows resources");
    }
    println!("cargo:rerun-if-changed=assets/rcwlp.ico");
}
