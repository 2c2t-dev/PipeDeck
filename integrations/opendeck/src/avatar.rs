//! A person's picture, as their Discord client fetched it: a PNG file,
//! made round and carried in a data URL, which a key's SVG and the touch
//! strip's layer both take.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

/// Pictures made round, by the file they came from. A person keeps one
/// file for as long as they keep one picture.
#[derive(Default)]
pub struct Avatars {
    round: HashMap<String, Option<String>>,
}

impl Avatars {
    /// The picture at `path`, round, as a PNG data URL; `None` for one that
    /// cannot be read, which is not tried again.
    pub fn get(&mut self, path: &str) -> Option<&str> {
        self.round
            .entry(path.to_owned())
            .or_insert_with(|| {
                round(path)
                    .map_err(|e| eprintln!("pipedeck: cannot read the picture {path}: {e}"))
                    .ok()
            })
            .as_deref()
    }
}

/// Read a PNG, cut it to a circle, and give it back as a data URL.
fn round(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("too large")?];
    let info = reader.next_frame(&mut buffer)?;
    let (width, height) = (info.width as usize, info.height as usize);
    let pixels = &buffer[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("a palette left unexpanded".into()),
    };
    let circle = circle(rgba, width, height);
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&circle)?;
    }
    Ok(format!(
        "data:image/png;base64,{}",
        crate::draw::base64(&out)
    ))
}

/// Make everything outside the circle the picture fits in transparent,
/// with a soft edge a pixel wide.
fn circle(mut rgba: Vec<u8>, width: usize, height: usize) -> Vec<u8> {
    let r = width.min(height) as f32 / 2.0;
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    for y in 0..height {
        for x in 0..width {
            let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let keep = (r - distance + 0.5).clamp(0.0, 1.0);
            let alpha = &mut rgba[(y * width + x) * 4 + 3];
            *alpha = (f32::from(*alpha) * keep).round() as u8;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corners_go_and_the_middle_stays() {
        let round = circle(vec![255; 8 * 8 * 4], 8, 8);
        let alpha = |x: usize, y: usize| round[(y * 8 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(4, 4), 255);
    }

    #[test]
    fn a_picture_is_read_made_round_and_carried() {
        let path = std::env::temp_dir().join(format!("pipedeck-avatar-{}.png", std::process::id()));
        {
            let file = File::create(&path).unwrap();
            let mut encoder = png::Encoder::new(file, 4, 4);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[200; 48])
                .unwrap();
        }
        let mut avatars = Avatars::default();
        let url = avatars.get(path.to_str().unwrap()).map(str::to_owned);
        assert!(url.is_some_and(|url| url.starts_with("data:image/png;base64,iVBOR")));
        assert!(avatars.get("/nowhere.png").is_none());
        let _ = std::fs::remove_file(path);
    }
}
