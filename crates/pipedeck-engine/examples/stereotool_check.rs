//! Runs a tone through the installed Stereo Tool and says what came out.
//!
//! `cargo run -p pipedeck-engine --example stereotool_check [preset.sts]`,
//! with `PIPEDECK_STEREOTOOL` pointing at the library if it was not
//! imported. This is the host on its own, with no PipeWire in the way: if
//! Stereo Tool works here, what remains is the routing.
//!
//! The licence key comes from `PIPEDECK_STEREOTOOL_LICENSE`, so a key is
//! never written down in the repository.

use std::path::PathBuf;
use std::process::ExitCode;

use pipedeck_engine::stereotool;

const BLOCK: usize = 512;

/// Loudness of a block, as the meters measure it.
fn peak(buffer: &[f32]) -> f32 {
    buffer
        .iter()
        .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()))
}

fn main() -> ExitCode {
    env_logger::init();
    let preset = std::env::args().nth(1).map(PathBuf::from);
    let license = std::env::var("PIPEDECK_STEREOTOOL_LICENSE").ok();

    let Some(path) = stereotool::library_path() else {
        println!("Stereo Tool is not installed. Import it, or set PIPEDECK_STEREOTOOL.");
        return ExitCode::FAILURE;
    };
    println!("{}", path.display());

    let mut instance =
        match stereotool::Instance::with_block(preset.as_deref(), license.as_deref(), BLOCK) {
            Ok(instance) => instance,
            Err(e) => {
                println!("[FAIL] {e}");
                return ExitCode::FAILURE;
            }
        };
    println!("[ ok ] it opened in blocks of {BLOCK}");
    println!(
        "[ ok ] licence {}, delay {} frames",
        if instance.licensed() {
            "valid".to_owned()
        } else {
            format!(
                "missing for {}",
                instance
                    .unlicensed_features()
                    .unwrap_or_else(|| "everything it runs".to_owned())
            )
        },
        instance.latency()
    );

    // A second of a 440 Hz tone, pushed through block by block. A processor
    // holds the first blocks back, which is what its delay means, so the
    // whole second is measured rather than one block.
    let rate = stereotool::SAMPLE_RATE as f32;
    let mut sent = 0.0f32;
    let mut back = 0.0f32;
    let mut frame = 0usize;
    for _ in 0..(stereotool::SAMPLE_RATE as usize / BLOCK) {
        let mut left: Vec<f32> = (0..BLOCK)
            .map(|offset| {
                let phase = (frame + offset) as f32 / rate * 440.0 * std::f32::consts::TAU;
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
    println!("STEREO TOOL OK");
    ExitCode::SUCCESS
}
