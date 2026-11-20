use crate::circuit::Circuit;
use crate::parts::{Part, Unit};

/// Shared electrical state every voice sees on a given sample.
#[derive(Clone, Copy, Debug)]
pub struct Bus {
    /// Accent line voltage relative to unloaded (drops as more voices draw on it).
    pub accent: f64,
    /// Supply rail relative to nominal.
    pub rail: f64,
    /// Summed output of the six shared square-wave oscillators (the "metal"
    /// source the hi-hat and cymbal both filter). Only computed while one of
    /// those two voices is awake; 0 otherwise.
    pub metal: f64,
    /// Gain for behavioral white-noise sources, which keeps their level in
    /// the audio band independent of the internal sample rate (1 at the
    /// default oversampling).
    pub noise: f64,
}

impl Default for Bus {
    fn default() -> Self {
        Self {
            accent: 1.0,
            rail: 1.0,
            metal: 0.0,
            noise: 1.0,
        }
    }
}

pub trait Voice {
    fn parts(&self) -> &[Part];
    fn parts_mut(&mut self) -> &mut [Part];
    /// Knob (0..1). Id 0 (level) is handled by the engine.
    fn set_knob(&mut self, id: u32, value: f64);
    /// Panel switch (e.g. tom/conga). Takes effect on the next trigger, so a
    /// ringing note is not retuned mid-decay. Voices without a switch ignore it.
    fn set_switch(&mut self, _value: u32) {}
    /// Push part values (after tolerance/aging) into the circuit.
    fn refresh(&mut self, unit: &Unit);
    /// `variant` selects closed/open on the hi-hat voice.
    fn trigger(&mut self, accent: f64, variant: u32);
    /// Advance one sample at the internal (oversampled) rate.
    fn tick(&mut self, bus: &Bus) -> f64;
    fn active(&self) -> bool;
    /// Accent-weighted trigger inputs currently loading the accent line.
    fn accent_draw(&self) -> f64;
    /// Current drawn from the supply rail (amps).
    fn supply_current(&self) -> f64;
}

/// Rectangular trigger pulse, like the 808's ~1 ms trigger with accent level.
#[derive(Default)]
pub struct Pulse {
    remaining: u32,
    accent: f64,
    width: u32,
}

impl Pulse {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            remaining: 0,
            accent: 0.0,
            width: (sample_rate * 1e-3) as u32,
        }
    }
    pub fn fire(&mut self, accent: f64) {
        self.accent = accent.clamp(0.0, 1.0);
        self.remaining = self.width;
    }
    /// Trigger voltage: ~4 V base plus the accent line (up to ~+8 V), both
    /// following the supply rail.
    pub fn next(&mut self, bus: &Bus) -> f64 {
        if self.remaining > 0 {
            self.remaining -= 1;
            (4.0 + 8.0 * self.accent * bus.accent) * bus.rail
        } else {
            0.0
        }
    }
    pub fn draw(&self) -> f64 {
        if self.remaining > 0 {
            self.accent
        } else {
            0.0
        }
    }
}

/// Puts a voice to sleep once every node has decayed below a threshold, so
/// silent voices cost nothing.
pub struct Sleeper {
    quiet: u32,
    limit: u32,
    pub awake: bool,
}

impl Sleeper {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            quiet: 0,
            limit: (sample_rate * 0.05) as u32,
            awake: false,
        }
    }
    pub fn wake(&mut self) {
        self.awake = true;
        self.quiet = 0;
    }
    pub fn observe(&mut self, circuits: &[&Circuit]) {
        let e = circuits.iter().fold(0.0f64, |m, c| m.max(c.energy()));
        self.observe_level(e);
    }
    pub fn observe_level(&mut self, e: f64) {
        if e < 2e-5 {
            self.quiet += 1;
            if self.quiet > self.limit {
                self.awake = false;
            }
        } else {
            self.quiet = 0;
        }
    }
}

/// Rough supply draw of a voice: quiescent current plus signal swing into ~2.2 kΩ.
pub fn supply_draw(active: bool, circuits: &[&Circuit]) -> f64 {
    if !active {
        return 0.0;
    }
    4e-3 + circuits.iter().map(|c| c.energy()).sum::<f64>() / 2.2e3
}

/// Map a 0..1 knob through an audio (log-ish) taper.
pub fn audio_taper(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    (x * x * 0.85 + x * 0.15).clamp(0.0, 1.0)
}
