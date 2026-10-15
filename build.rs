fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
    // The icon and the manifest: per-monitor DPI awareness and Windows 10/11 compatibility. It doesn't ask for
    // administrator rights, which the app needs (the EC is reached through the BIOS WMI interface): a launch without
    // them opens the running app, or starts it with them (see shell::instance::open_elevated).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("res/app.rc", embed_resource::NONE).manifest_required().unwrap();
    }
    println!("cargo:rerun-if-changed=res");
}
