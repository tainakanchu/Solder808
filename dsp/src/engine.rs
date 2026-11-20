use crate::bd::BassDrum;
use crate::cb::Cowbell;
use crate::common::Common;
use crate::cp::Clap;
use crate::hh::HiHat;
use crate::parts::{Part, Unit};
use crate::sd::Snare;
use crate::voice::Voice;

pub const OVERSAMPLE: usize = 2;
pub const BLOCK: usize = 128;
pub const VOICES: usize = 5;

/// Output trim per voice so circuit volts land near ±1.
const TRIM: [f64; VOICES] = [0.85, 0.1, 0.3, 0.7, 0.18];

/// Trigger slots exposed to the UI.
#[derive(Clone, Copy)]
pub enum Slot {
    Bd = 0,
    Sd = 1,
    Ch = 2,
    Oh = 3,
    Cp = 4,
    Cb = 5,
}

impl Slot {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Slot::Bd,
            1 => Slot::Sd,
            2 => Slot::Ch,
            3 => Slot::Oh,
            4 => Slot::Cp,
            5 => Slot::Cb,
            _ => return None,
        })
    }
}

struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn lowpass(sr: f64, fc: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * fc / sr;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - c) / 2.0 / a0,
            b1: (1.0 - c) / a0,
            b2: (1.0 - c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

pub struct Engine {
    voices: [Box<dyn Voice>; VOICES],
    common: Common,
    unit: Unit,
    aa: [Biquad; 2],
    pub out: [f32; BLOCK],
    pub master: f64,
}

/// Index used for the shared circuitry in part-table calls.
pub const COMMON: usize = VOICES;

impl Engine {
    pub fn new(sample_rate: f64) -> Self {
        let isr = sample_rate * OVERSAMPLE as f64;
        let fc = sample_rate * 0.45;
        Self {
            voices: [
                Box::new(BassDrum::new(isr)),
                Box::new(Snare::new(isr)),
                Box::new(HiHat::new(isr)),
                Box::new(Clap::new(isr)),
                Box::new(Cowbell::new(isr)),
            ],
            common: Common::new(isr),
            unit: Unit::default(),
            aa: [
                Biquad::lowpass(isr, fc, 0.541_196),
                Biquad::lowpass(isr, fc, 1.306_563),
            ],
            out: [0.0; BLOCK],
            master: 0.8,
        }
    }

    /// Parts of a voice (0..VOICES) or of the shared circuitry (`COMMON`).
    pub fn parts(&self, v: usize) -> Option<&[Part]> {
        if v == COMMON {
            Some(self.common.parts())
        } else {
            self.voices.get(v).map(|b| b.parts())
        }
    }

    pub fn trigger(&mut self, slot: Slot, accent: f64) {
        match slot {
            Slot::Bd => self.voices[0].trigger(accent, 0),
            Slot::Sd => self.voices[1].trigger(accent, 0),
            Slot::Ch => self.voices[2].trigger(accent, 0),
            Slot::Oh => self.voices[2].trigger(accent, 1),
            Slot::Cp => self.voices[3].trigger(accent, 0),
            Slot::Cb => self.voices[4].trigger(accent, 0),
        }
    }

    pub fn set_knob(&mut self, voice: usize, id: u32, value: f64) {
        let Some(v) = self.voices.get_mut(voice) else {
            return;
        };
        if id == 0 {
            self.common.set_level(voice, value);
        } else {
            v.set_knob(id, value.clamp(0.0, 1.0));
            v.refresh(&self.unit);
        }
    }

    pub fn set_accent(&mut self, value: f64) {
        self.common.accent_knob = value.clamp(0.0, 1.0);
    }

    pub fn set_unit(&mut self, unit: Unit) {
        self.unit = unit;
        for v in &mut self.voices {
            v.refresh(&unit);
        }
        self.common.refresh(&unit);
    }

    pub fn set_part_scale(&mut self, voice: usize, part: usize, scale: f64) {
        let unit = self.unit;
        let scale = scale.clamp(0.05, 20.0);
        if voice == COMMON {
            if let Some(p) = self.common.parts_mut().get_mut(part) {
                p.user_scale = scale;
            }
            self.common.refresh(&unit);
        } else if let Some(v) = self.voices.get_mut(voice) {
            if let Some(p) = v.parts_mut().get_mut(part) {
                p.user_scale = scale;
            }
            v.refresh(&unit);
        }
    }

    /// Render `frames` samples (≤ BLOCK) into `self.out`.
    pub fn process(&mut self, frames: usize) {
        let frames = frames.min(BLOCK);
        for i in 0..frames {
            let mut y = 0.0;
            for _ in 0..OVERSAMPLE {
                let draw: f64 = self.voices.iter().map(|v| v.accent_draw()).sum();
                let bus = self.common.bus(draw);
                let mut outs = [0.0; VOICES];
                let mut current = 0.0;
                let mut any = false;
                for (k, v) in self.voices.iter_mut().enumerate() {
                    if v.active() {
                        outs[k] = v.tick(&bus) * TRIM[k];
                        current += v.supply_current();
                        any = true;
                    }
                }
                self.common.update_rail(current);
                let s = self.common.mix(&outs, any, bus.rail);
                let z = self.aa[0].run(s);
                y = self.aa[1].run(z);
            }
            // Converter-side soft limit, then master.
            self.out[i] = ((y * self.master).tanh()) as f32;
        }
    }

    pub fn rail_volts(&self) -> f64 {
        self.common.rail_volts()
    }

    pub fn any_active(&self) -> bool {
        self.voices.iter().any(|v| v.active())
    }
}

#[cfg(test)]
mod fuzz {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn u(&mut self, n: usize) -> usize {
            (self.f() * n as f64) as usize
        }
    }

    /// Random UI-reachable settings and hits must never produce non-finite
    /// output. Run with `FUZZ_CASES=500 cargo test fuzz -- --nocapture` for more.
    #[test]
    fn random_settings_stay_finite() {
        let cases: u64 = std::env::var("FUZZ_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(40);
        let sr = 48_000.0;
        for case in 1..=cases {
            let mut r = Rng(case.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut e = Engine::new(sr);
            let mut log = Vec::new();
            for blk in 0..(sr as usize * 3 / BLOCK) {
                if r.f() < 0.05 {
                    match r.u(5) {
                        0 => {
                            let (v, id, x) = (r.u(VOICES), r.u(4) as u32, r.f());
                            e.set_knob(v, id, x);
                            log.push(format!("knob {v} {id} {x:.3}"));
                        }
                        1 => {
                            let v = r.u(VOICES + 1);
                            let n = e.parts(v).map_or(0, |p| p.len());
                            if n > 0 {
                                let (i, s) = (r.u(n), 2f64.powf(r.f() * 4.0 - 2.0));
                                e.set_part_scale(v, i, s);
                                log.push(format!("part {v} {i} {s:.3}"));
                            }
                        }
                        2 => {
                            let u = Unit {
                                seed: r.u(1 << 20) as u32,
                                age_years: (r.f() * 50.0).round(),
                                temp_c: (r.f() * 45.0).round(),
                            };
                            e.set_unit(u);
                            log.push(format!("unit {} {} {}", u.seed, u.age_years, u.temp_c));
                        }
                        3 => {
                            let x = r.f();
                            e.set_accent(x);
                            log.push(format!("accent {x:.3}"));
                        }
                        _ => {}
                    }
                }
                if blk % 4 == 0 {
                    for s in 0..6 {
                        if r.f() < 0.3 {
                            e.trigger(
                                Slot::from_u32(s).unwrap(),
                                if r.f() < 0.3 { 1.0 } else { 0.35 },
                            );
                        }
                    }
                }
                e.process(BLOCK);
                if let Some(i) = e.out.iter().position(|y| !y.is_finite()) {
                    panic!(
                        "case {case} block {blk} sample {i} non-finite; recent: {:?}",
                        &log[log.len().saturating_sub(6)..]
                    );
                }
            }
            // Once the hits stop, no circuit may keep Newton at its cap (that
            // is what pushed the browser past 100 % load).
            crate::circuit::stats::take();
            for _ in 0..(sr as usize * 2 / BLOCK) {
                e.process(BLOCK);
            }
            let (_, _, maxed) = crate::circuit::stats::take();
            assert!(
                maxed < 50,
                "case {case}: {maxed} capped Newton steps in silence; recent: {:?}",
                &log[log.len().saturating_sub(6)..]
            );
        }
    }
}

#[cfg(test)]
mod long_play {
    use super::*;

    /// Plays the PPAP preset for `LONG_SECS` (default 600) and reports drift.
    /// `cargo test --release long_play -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn ppap_keeps_sounding() {
        let secs: usize = std::env::var("LONG_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600);
        let sr = 48_000.0;
        let rows = [
            "x.......x.......x.....x...x.x...",
            "....x.......x.......x.......x...",
            "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.",
            "................................",
            "................................",
            "....x.......x.......x.......x...",
        ];
        let mut e = Engine::new(sr);
        for (v, id, x) in [
            (0, 3, 0.76),
            (0, 1, 0.23),
            (1, 0, 0.84),
            (1, 2, 0.74),
            (2, 0, 0.7),
            (2, 1, 0.21),
            (4, 0, 0.79),
        ] {
            e.set_knob(v, id, x);
        }
        let step_len = sr * 60.0 / 136.0 / 4.0;
        let (mut until, mut step) = (0.0f64, 0usize);
        let report = sr as usize * 30 / BLOCK;
        let (mut peak, mut worst) = (0f32, 0f64);
        crate::circuit::stats::take();
        for blk in 0..(sr as usize * secs / BLOCK) {
            let t = std::time::Instant::now();
            let mut done = 0;
            while done < BLOCK {
                if until < 1.0 {
                    for (s, row) in rows.iter().enumerate() {
                        if row.as_bytes()[step] == b'x' {
                            e.trigger(Slot::from_u32(s as u32).unwrap(), 0.35);
                        }
                    }
                    step = (step + 1) % 32;
                    until += step_len;
                }
                let n = (BLOCK - done).min(until.ceil() as usize).max(1);
                e.process(n);
                assert!(
                    e.out[..n].iter().all(|y| y.is_finite()),
                    "non-finite at block {blk}"
                );
                peak = e.out[..n].iter().fold(peak, |m, y| m.max(y.abs()));
                done += n;
                until -= n as f64;
            }
            worst = worst.max(t.elapsed().as_secs_f64() * 1e3);
            if (blk + 1) % report == 0 {
                let (steps, its, maxed) = crate::circuit::stats::take();
                println!(
                    "{:4} s: peak {peak:.3}, worst block {worst:.3} ms, iters/step {:.2}, maxed {maxed}, rail {:.2} V, awake {:?}",
                    (blk + 1) * BLOCK / sr as usize,
                    its as f64 / steps.max(1) as f64,
                    e.rail_volts(),
                    (0..VOICES).filter(|&k| e.voices[k].active()).collect::<Vec<_>>(),
                );
                assert!(peak > 0.05, "went silent");
                peak = 0.0;
                worst = 0.0;
            }
        }
    }
}
