//! Runs a tone through an installed VST3 effect and says what came out.
//!
//! `cargo run -p pipedeck-engine --example plugin_check [name]`, with
//! `VST3_PATH` pointing at a directory of bundles if they are not installed.
//! This is the plug-in host on its own, with no PipeWire in the way: if a
//! plug-in works here, what remains is the routing.

use std::process::ExitCode;

use pipedeck_engine::vst3;

const RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

/// Loudness of a block, as the meters measure it.
fn peak(buffer: &[f32]) -> f32 {
    buffer
        .iter()
        .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()))
}

fn main() -> ExitCode {
    env_logger::init();
    let wanted = std::env::args().nth(1);

    let plugins = vst3::installed();
    let Some(plugin) = plugins.iter().find(|plugin| match &wanted {
        Some(name) => plugin.name.to_lowercase().contains(&name.to_lowercase()),
        None => true,
    }) else {
        println!("No VST3 effect found. Point VST3_PATH at a directory of bundles.");
        return ExitCode::FAILURE;
    };

    println!("{} by {}", plugin.name, plugin.vendor);
    let instance = match vst3::Instance::open(plugin, RATE, BLOCK) {
        Ok(instance) => instance,
        Err(e) => {
            println!("[FAIL] {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("[ ok ] it opened at {RATE} Hz in blocks of {BLOCK}");

    // A second of a 440 Hz tone, pushed through block by block.
    let mut sent = 0.0f32;
    let mut back = 0.0f32;
    let mut frame = 0usize;
    for _ in 0..(RATE as usize / BLOCK) {
        let mut left: Vec<f32> = (0..BLOCK)
            .map(|offset| {
                let phase = (frame + offset) as f32 / RATE as f32 * 440.0 * std::f32::consts::TAU;
                phase.sin() * 0.5
            })
            .collect();
        let mut right = left.clone();
        frame += BLOCK;
        sent = sent.max(peak(&left));

        let mut channels: Vec<&mut [f32]> = vec![&mut left, &mut right];
        if instance.process(&mut channels).is_err() {
            println!("[FAIL] it refused to process a block");
            return ExitCode::FAILURE;
        }
        back = back.max(peak(&left));
    }

    println!("[ ok ] a second of tone went through: in {sent:.3}, out {back:.3}");
    if back <= 0.0 {
        println!("[FAIL] nothing came out");
        return ExitCode::FAILURE;
    }
    println!("PLUGIN OK");
    ExitCode::SUCCESS
}
