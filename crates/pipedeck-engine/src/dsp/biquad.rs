//! Second-order filters, from Robert Bristow-Johnson's cookbook.
//!
//! One set of coefficients describes a filter; a state runs it over one
//! channel. The coefficients also say what the filter does at a frequency,
//! which is how the equaliser's curve is drawn from the same numbers the
//! audio goes through.

use std::f32::consts::{PI, SQRT_2};

use super::SAMPLE_RATE;

/// Normalised coefficients: `a0` is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeffs {
    /// A filter that changes nothing.
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalise(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn omega(freq: f32) -> (f32, f32) {
        let w = 2.0 * PI * freq.clamp(10.0, SAMPLE_RATE * 0.49) / SAMPLE_RATE;
        (w.cos(), w.sin())
    }

    /// A bell around `freq`, `gain_db` high, `q` narrow.
    pub fn peaking(freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / (2.0 * q.max(0.05));
        Self::normalise(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// Everything under `freq` raised or lowered by `gain_db`.
    pub fn low_shelf(freq: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / SQRT_2;
        let root = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) - (a - 1.0) * cos + root),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - root),
            (a + 1.0) + (a - 1.0) * cos + root,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - root,
        )
    }

    /// Everything over `freq` raised or lowered by `gain_db`.
    pub fn high_shelf(freq: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / SQRT_2;
        let root = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) + (a - 1.0) * cos + root),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - root),
            (a + 1.0) - (a - 1.0) * cos + root,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - root,
        )
    }

    /// Everything under `freq` taken away, twelve decibels an octave.
    pub fn highpass(freq: f32) -> Self {
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / SQRT_2;
        Self::normalise(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Everything over `freq` taken away, twelve decibels an octave.
    pub fn lowpass(freq: f32) -> Self {
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / SQRT_2;
        Self::normalise(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// A band around `freq`, flat on top at unity.
    pub fn bandpass(freq: f32, q: f32) -> Self {
        let (cos, sin) = Self::omega(freq);
        let alpha = sin / (2.0 * q.max(0.05));
        Self::normalise(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// What the filter does at `freq`, in decibels.
    pub fn response_db(&self, freq: f32) -> f32 {
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let num = num_re * num_re + num_im * num_im;
        let den = (den_re * den_re + den_im * den_im).max(1e-20);
        10.0 * (num / den).max(1e-20).log10()
    }
}

/// The memory of a filter over one channel.
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    z1: f32,
    z2: f32,
}

impl State {
    /// One sample through, in transposed direct form II.
    #[inline]
    pub fn run(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bell_lifts_its_own_frequency_and_leaves_the_rest() {
        let bell = Coeffs::peaking(1000.0, 1.0, 6.0);
        assert!((bell.response_db(1000.0) - 6.0).abs() < 0.1);
        assert!(bell.response_db(50.0).abs() < 0.3);
        assert!(bell.response_db(15000.0).abs() < 0.3);
    }

    #[test]
    fn shelves_and_a_low_cut_do_what_they_say() {
        let low = Coeffs::low_shelf(200.0, -6.0);
        assert!((low.response_db(30.0) + 6.0).abs() < 0.5);
        assert!(low.response_db(5000.0).abs() < 0.3);
        let high = Coeffs::high_shelf(8000.0, 4.0);
        assert!((high.response_db(20000.0) - 4.0).abs() < 0.5);
        assert!(high.response_db(200.0).abs() < 0.3);
        let cut = Coeffs::highpass(100.0);
        assert!(cut.response_db(25.0) < -20.0);
        assert!(cut.response_db(1000.0).abs() < 0.3);
        assert!(Coeffs::IDENTITY.response_db(440.0).abs() < 1e-3);
    }
}
