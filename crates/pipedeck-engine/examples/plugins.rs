//! Lists the VST3 effects installed on this machine.
//!
//! Run with `cargo run -p pipedeck-engine --example plugins`. It loads every
//! bundle it finds, which runs the plug-ins' own load-time code.

fn main() {
    env_logger::init();

    println!("Looking in:");
    for path in pipedeck_engine::vst3::search_paths() {
        println!("  {}", path.display());
    }

    let bundles = pipedeck_engine::vst3::bundles();
    println!("\n{} bundle(s) found", bundles.len());

    let plugins = pipedeck_engine::vst3::installed();
    if plugins.is_empty() {
        println!("No VST3 effect installed.");
        return;
    }
    println!("\n{} effect(s):", plugins.len());
    for plugin in plugins {
        println!("  {:<34} {}", plugin.name, plugin.class_id);
        println!("  {:<34} {}", "", plugin.vendor);
    }
}
