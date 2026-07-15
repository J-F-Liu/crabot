// build.rs — Embed assets/images/icon.ico as a Windows executable icon resource.

use std::env;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "windows" {
        return;
    }

    println!("cargo:rerun-if-changed=assets/images/icon.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/images/icon.ico");
    res.compile().unwrap();
}
