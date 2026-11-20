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
    /// Advance one sample at the internal (oversampled) rate. Only called
    /// while `active`.
    fn tick(&mut self, bus: &Bus) -> f64;
    /// Computing audio (asleep or coasting otherwise).
    fn active(&self) -> bool;
    /// Every [`book_every`] internal samples, awake or not: sleep decisions
    /// and coasting.
    fn housekeep(&mut self);
    /// Output level (V at the voice output) the voice may sleep below.
    fn set_quiet(&mut self, volts: f64);
    /// Accent-weighted trigger inputs currently loading the accent line.
    fn accent_draw(&self) -> f64;
    /// Current drawn from the supply rail beyond the quiescent current (A).
    fn supply_current(&self) -> f64;
}

/// Test-only switches for comparing runs that differ only in sleeping.
#[cfg(test)]
pub mod testing {
    use std::cell::Cell;
    thread_local! {
        /// Voices never go to sleep.
        pub static NEVER_SLEEP: Cell<bool> = const { Cell::new(false) };
        /// Noise generators and oscillators restart on every trigger (the
        /// shared metal bank on every HH or CY trigger).
        pub static RESTART_SOURCES: Cell<bool> = const { Cell::new(false) };
    }

    pub fn restart_sources() -> bool {
        RESTART_SOURCES.with(|f| f.get())
    }

    pub fn never_sleep() -> bool {
        NEVER_SLEEP.with(|f| f.get())
    }
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
    /// The pulse is still high.
    pub fn busy(&self) -> bool {
        self.remaining > 0
    }
    pub fn draw(&self) -> f64 {
        if self.remaining > 0 {
            self.accent
        } else {
            0.0
        }
    }
}

/// Housekeeping cadence: voices decide about sleeping, and the engine
/// samples the supply draw, every this many internal samples (32 at 96 kHz,
/// 16 at 48 kHz). Coasting circuits step at this multiple of the time step.
pub fn book_every(sample_rate: f64) -> usize {
    ((sample_rate / 3000.0).round() as usize).max(1)
}

/// How long a voice must stay silent before it stops computing audio (s).
const HOLD: f64 = 0.05;
/// A coasting circuit stops once no node is further than this from rest (V).
const REST: f64 = 1e-6;

/// Stops computing a voice once it is silent, without changing its next note.
///
/// A voice is silent once only a trigger can make it louder (its pulse is
/// over, its VCAs closed) and its output has stayed below the level the
/// engine sets with [`Sleeper::set_threshold`] (−90 dBFS at the engine output,
/// after trim, at full level) for 50 ms. Its envelope capacitors may still
/// hold charge then, which would change the next note if the state were
/// simply frozen; so the voice coasts instead: copies of its circuits, at a
/// time step [`book_every`] times longer (backward Euler) and with every
/// voltage source at rest, carry on decaying at a small fraction of the cost
/// until every node is within 1 µV of rest. A trigger loads their state back.
pub struct Sleeper {
    awake: bool,
    /// The coarse copies hold the state (the voice's own circuits are stale).
    coasting: bool,
    /// The coarse copies are still being stepped.
    settling: bool,
    quiet: u32,
    hold: u32,
    /// Peak output since the last housekeeping.
    peak: f64,
    out_thr: f64,
    coarse: Vec<Circuit>,
}

impl Sleeper {
    /// `circuits`: the voice's circuits this sleeper covers, in the order
    /// later calls pass them.
    pub fn new(sample_rate: f64, circuits: &[&Circuit]) -> Self {
        let every = book_every(sample_rate);
        Self {
            awake: false,
            coasting: false,
            settling: false,
            quiet: 0,
            hold: (sample_rate * HOLD / every as f64).ceil() as u32,
            peak: 0.0,
            out_thr: 2e-5,
            coarse: circuits.iter().map(|c| c.coarse(every)).collect(),
        }
    }

    /// Computing audio.
    pub fn awake(&self) -> bool {
        self.awake
    }

    /// Fully at rest: neither computing audio nor coasting.
    pub fn resting(&self) -> bool {
        !self.awake && !self.settling
    }

    /// Output level (V at the voice output) below which the voice is silent.
    pub fn set_threshold(&mut self, volts: f64) {
        self.out_thr = volts;
    }

    /// Pass the voice's output through (every sample while awake).
    #[inline]
    pub fn hear(&mut self, y: f64) -> f64 {
        self.peak = self.peak.max(y.abs());
        y
    }

    /// A trigger: compute audio again, resuming from the coasted state.
    pub fn wake(&mut self, circuits: &mut [&mut Circuit]) {
        self.restore(circuits);
        self.coasting = false;
        self.settling = false;
        self.awake = true;
        self.quiet = 0;
        self.peak = 0.0;
    }

    /// Every [`book_every`] samples, awake or not. `idle`: only a trigger
    /// could make the voice louder.
    pub fn housekeep(&mut self, circuits: &[&Circuit], idle: bool) {
        if self.awake {
            let peak = std::mem::take(&mut self.peak);
            #[cfg(test)]
            if testing::never_sleep() {
                return;
            }
            if idle && peak < self.out_thr {
                self.quiet += 1;
                if self.quiet >= self.hold {
                    self.awake = false;
                    self.coasting = true;
                    self.settling = true;
                    for (c, f) in self.coarse.iter_mut().zip(circuits) {
                        c.coast_from(f);
                    }
                }
            } else {
                self.quiet = 0;
            }
        } else if self.settling {
            let mut e = 0.0f64;
            for c in &mut self.coarse {
                c.step();
                e = e.max(c.energy());
            }
            self.settling = e > REST;
        }
    }

    /// Before the voice changes element values: bring the coasted state back.
    pub fn restore(&mut self, circuits: &mut [&mut Circuit]) {
        if self.coasting {
            for (f, c) in circuits.iter_mut().zip(&self.coarse) {
                f.load_state(c);
            }
        }
    }

    /// After the voice changed element values: coast on with them.
    pub fn resume(&mut self, circuits: &[&Circuit]) {
        if self.coasting {
            for (c, f) in self.coarse.iter_mut().zip(circuits) {
                c.coast_from(f);
            }
        }
    }
}

/// Rough supply draw of a voice beyond its quiescent current: signal swing
/// into ~2.2 kΩ. The quiescent current flows whether or not the voice is
/// being computed, so it is part of the nominal rail rather than something
/// a sleeping voice could stop drawing.
pub fn supply_draw(active: bool, circuits: &[&Circuit]) -> f64 {
    if !active {
        return 0.0;
    }
    circuits.iter().map(|c| c.energy()).sum::<f64>() / 2.2e3
}

/// Map a 0..1 knob through an audio (log-ish) taper.
pub fn audio_taper(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    (x * x * 0.85 + x * 0.15).clamp(0.0, 1.0)
}
