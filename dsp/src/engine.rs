use crate::bd::BassDrum;
use crate::cb::Cowbell;
use crate::common::Common;
use crate::cp::Clap;
use crate::cy::Cymbal;
use crate::hh::{HiHat, MetalBank};
use crate::parts::{Part, Unit};
use crate::rs::RimClaves;
use crate::sd::Snare;
use crate::tom::{TomConga, HIGH, LOW, MID};
use crate::voice::Voice;

/// Default internal oversampling factor (1 or 2, switchable at run time with
/// [`Engine::set_oversample`]).
pub const OVERSAMPLE: usize = 2;
pub const BLOCK: usize = 128;
/// Length of the fade that hides the cut when the oversampling changes (s).
const SWITCH_FADE: f64 = 0.005;
/// Voice (channel) indices: 0 BD, 1 SD, 2 HH, 3 CP/MA, 4 CB, 5 LT/LC,
/// 6 MT/MC, 7 HT/HC, 8 RS/CL, 9 CY. Also the part-table index.
pub const VOICES: usize = 10;
const HH: usize = 2;
const CY: usize = 9;

/// Output trim per voice so circuit volts land near ±1 (BD stays the loudest).
const TRIM: [f64; VOICES] = [0.85, 0.1, 0.3, 0.7, 0.18, 0.15, 0.15, 0.15, 0.38, 0.43];

/// Trigger slots exposed to the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Bd = 0,
    Sd = 1,
    Ch = 2,
    Oh = 3,
    /// Clap or maracas (voice 3's switch).
    Cp = 4,
    Cb = 5,
    /// Low tom or low conga (voice 5's switch).
    Lt = 6,
    Mt = 7,
    Ht = 8,
    /// Rim shot or claves (voice 8's switch).
    Rs = 9,
    Cy = 10,
}

pub const SLOTS: u32 = 11;

impl Slot {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Slot::Bd,
            1 => Slot::Sd,
            2 => Slot::Ch,
            3 => Slot::Oh,
            4 => Slot::Cp,
            5 => Slot::Cb,
            6 => Slot::Lt,
            7 => Slot::Mt,
            8 => Slot::Ht,
            9 => Slot::Rs,
            10 => Slot::Cy,
            _ => return None,
        })
    }

    /// (voice index, variant) this slot strikes.
    fn target(self) -> (usize, u32) {
        match self {
            Slot::Bd => (0, 0),
            Slot::Sd => (1, 0),
            Slot::Ch => (2, 0),
            Slot::Oh => (2, 1),
            Slot::Cp => (3, 0),
            Slot::Cb => (4, 0),
            Slot::Lt => (5, 0),
            Slot::Mt => (6, 0),
            Slot::Ht => (7, 0),
            Slot::Rs => (8, 0),
            Slot::Cy => (9, 0),
        }
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
    metal: MetalBank,
    sr: f64,
    os: usize,
    isr: f64,
    common: Common,
    unit: Unit,
    /// Decimation filter (2× only; at 1× there is nothing to decimate).
    aa: [Biquad; 2],
    /// Knob values (ids 1..=3) and panel switches as last set, so a rebuild
    /// for another oversampling factor can restore them.
    knobs: [[Option<f64>; 4]; VOICES],
    switches: [Option<u32>; VOICES],
    /// Behavioral noise sources' gain, so their in-band level (white noise
    /// spreads its power up to the internal Nyquist) is the same at 1× and 2×.
    noise: f64,
    /// Last output before master, and the fade that bridges a rebuild's cut.
    last: f64,
    fade: f64,
    fade_step: f64,
    pub out: [f32; BLOCK],
    pub master: f64,
}

/// Index used for the shared circuitry in part-table calls.
pub const COMMON: usize = VOICES;

fn build_voices(isr: f64) -> [Box<dyn Voice>; VOICES] {
    [
        Box::new(BassDrum::new(isr)),
        Box::new(Snare::new(isr)),
        Box::new(HiHat::new(isr)),
        Box::new(Clap::new(isr)),
        Box::new(Cowbell::new(isr)),
        Box::new(TomConga::new(isr, &LOW)),
        Box::new(TomConga::new(isr, &MID)),
        Box::new(TomConga::new(isr, &HIGH)),
        Box::new(RimClaves::new(isr)),
        Box::new(Cymbal::new(isr)),
    ]
}

fn decimator(sr: f64, isr: f64) -> [Biquad; 2] {
    let fc = sr * 0.45;
    [
        Biquad::lowpass(isr, fc, 0.541_196),
        Biquad::lowpass(isr, fc, 1.306_563),
    ]
}

impl Engine {
    pub fn new(sample_rate: f64) -> Self {
        let isr = sample_rate * OVERSAMPLE as f64;
        let mut e = Self {
            voices: build_voices(isr),
            metal: MetalBank::default(),
            sr: sample_rate,
            os: OVERSAMPLE,
            isr,
            common: Common::new(isr),
            unit: Unit::default(),
            aa: decimator(sample_rate, isr),
            knobs: [[None; 4]; VOICES],
            switches: [None; VOICES],
            noise: 1.0,
            last: 0.0,
            fade: 0.0,
            fade_step: 0.0,
            out: [0.0; BLOCK],
            master: 0.8,
        };
        e.retune_metal();
        e
    }

    /// Internal oversampling factor in use (1 or 2).
    pub fn oversample(&self) -> usize {
        self.os
    }

    /// Switch the internal oversampling factor (1 or 2; anything else is
    /// ignored). Takes effect from the next `process`. Every voice is rebuilt
    /// for the new internal rate, so pulse widths, sleep times, oscillator
    /// increments and every circuit's time step follow it; knobs, switches,
    /// the unit, part edits, levels, accent and master carry over. Notes still
    /// ringing are cut, behind a short fade from the last output sample.
    pub fn set_oversample(&mut self, factor: u32) {
        let os = factor as usize;
        if !(os == 1 || os == 2) || os == self.os {
            return;
        }
        self.os = os;
        self.isr = self.sr * os as f64;
        // White noise at the internal rate: same density in the audio band.
        self.noise = (os as f64 / OVERSAMPLE as f64).sqrt();
        let mut voices = build_voices(self.isr);
        for (k, v) in voices.iter_mut().enumerate() {
            for (p, old) in v.parts_mut().iter_mut().zip(self.voices[k].parts()) {
                p.user_scale = old.user_scale;
            }
            for (id, x) in self.knobs[k].iter().enumerate() {
                if let Some(x) = *x {
                    v.set_knob(id as u32, x);
                }
            }
            if let Some(sw) = self.switches[k] {
                v.set_switch(sw);
            }
            v.refresh(&self.unit);
        }
        self.voices = voices;
        self.common.rebuild(self.isr, &self.unit);
        self.aa = decimator(self.sr, self.isr);
        self.retune_metal();
        self.fade = self.last;
        self.fade_step = self.last / (self.sr * SWITCH_FADE).max(1.0);
    }

    /// The shared oscillator bank follows the hi-hat's R/C parts.
    fn retune_metal(&mut self) {
        self.metal.tune(self.voices[HH].parts(), self.isr);
    }

    /// Frequencies of the shared metal oscillators (Hz).
    pub fn metal_hz(&self) -> [f64; 6] {
        self.metal.hz(self.isr)
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
        let (v, variant) = slot.target();
        self.voices[v].trigger(accent, variant);
    }

    /// Panel switch of a voice: 3 clap/maracas, 5–7 tom/conga, 8 rim
    /// shot/claves (0 / 1). Applies from the next trigger; other voices ignore it.
    pub fn set_switch(&mut self, voice: usize, value: u32) {
        if let Some(v) = self.voices.get_mut(voice) {
            v.set_switch(value);
            self.switches[voice] = Some(value);
        }
    }

    pub fn set_knob(&mut self, voice: usize, id: u32, value: f64) {
        let Some(v) = self.voices.get_mut(voice) else {
            return;
        };
        if id == 0 {
            self.common.set_level(voice, value);
        } else {
            let value = value.clamp(0.0, 1.0);
            v.set_knob(id, value);
            v.refresh(&self.unit);
            if let Some(k) = self.knobs[voice].get_mut(id as usize) {
                *k = Some(value);
            }
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
        self.retune_metal();
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
            if voice == HH {
                self.retune_metal();
            }
        }
    }

    /// Render `frames` samples (≤ BLOCK) into `self.out`.
    pub fn process(&mut self, frames: usize) {
        let frames = frames.min(BLOCK);
        for i in 0..frames {
            let mut y = 0.0;
            for _ in 0..self.os {
                let draw: f64 = self.voices.iter().map(|v| v.accent_draw()).sum();
                let mut bus = self.common.bus(draw);
                bus.noise = self.noise;
                if self.voices[HH].active() || self.voices[CY].active() {
                    bus.metal = self.metal.tick();
                }
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
                y = if self.os > 1 {
                    let z = self.aa[0].run(s);
                    self.aa[1].run(z)
                } else {
                    s
                };
            }
            if self.fade != 0.0 {
                y += self.fade;
                self.fade -= self.fade_step;
                if self.fade * self.fade_step <= 0.0 {
                    self.fade = 0.0;
                }
            }
            self.last = y;
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
mod instruments {
    use super::*;

    const SR: f64 = 48_000.0;

    /// (slot, voice whose switch to set, switch value)
    const CASES: [(u32, usize, u32); 16] = [
        (0, 0, 0),
        (1, 0, 0),
        (2, 0, 0),
        (3, 0, 0),
        (4, 3, 0),
        (4, 3, 1),
        (5, 0, 0),
        (6, 5, 0),
        (6, 5, 1),
        (7, 6, 0),
        (7, 6, 1),
        (8, 7, 0),
        (8, 7, 1),
        (9, 8, 0),
        (9, 8, 1),
        (10, 0, 0),
    ];

    #[test]
    fn every_instrument_sounds_then_sleeps() {
        for (slot, v, sw) in CASES {
            let mut e = Engine::new(SR);
            e.set_switch(v, sw);
            e.trigger(Slot::from_u32(slot).unwrap(), 1.0);
            let mut peak = 0.0f32;
            for _ in 0..(SR as usize / BLOCK / 2) {
                e.process(BLOCK);
                assert!(e.out.iter().all(|y| y.is_finite()));
                peak = e.out.iter().fold(peak, |m, y| m.max(y.abs()));
            }
            assert!(peak > 0.1 && peak < 0.99, "slot {slot} sw {sw}: {peak}");
            crate::circuit::stats::take();
            for _ in 0..(SR as usize * 4 / BLOCK) {
                e.process(BLOCK);
            }
            let (_, _, maxed) = crate::circuit::stats::take();
            assert_eq!(maxed, 0, "slot {slot} sw {sw}");
            assert!(!e.any_active(), "slot {slot} sw {sw} never sleeps");
        }
    }

    /// A unit change on a silent machine makes no sound, and the first hit
    /// after it sounds like a hit on a machine built as that unit (the tom
    /// channels' standing-bias operating point used to keep the old unit's
    /// value until the first note had rung out). Every other voice rests at
    /// 0 V on every node, so only the toms carry a DC point to re-settle.
    #[test]
    fn first_hit_after_a_unit_change_matches_a_fresh_unit() {
        let rms = |e: &mut Engine| {
            let mut s = 0.0f64;
            let n = (0.3 * SR) as usize / BLOCK;
            for _ in 0..n {
                e.process(BLOCK);
                s += e.out.iter().map(|y| (*y as f64).powi(2)).sum::<f64>();
            }
            (s / (n * BLOCK) as f64).sqrt()
        };
        for u in [
            Unit {
                seed: 0,
                age_years: 0.0,
                temp_c: 0.0,
            },
            Unit {
                seed: 9,
                age_years: 40.0,
                temp_c: 45.0,
            },
        ] {
            for (slot, v, sw) in CASES {
                let slot = Slot::from_u32(slot).unwrap();
                let mut a = Engine::new(SR);
                a.set_switch(v, sw);
                a.trigger(slot, 0.5);
                for _ in 0..(SR as usize * 4 / BLOCK) {
                    a.process(BLOCK);
                }
                assert!(!a.any_active());
                a.set_unit(u);
                for _ in 0..(SR as usize / 2 / BLOCK) {
                    a.process(BLOCK);
                    assert!(a.out.iter().all(|y| *y == 0.0), "{slot:?} sw {sw}");
                }
                a.trigger(slot, 0.5);
                let mut b = Engine::new(SR);
                b.set_unit(u);
                b.set_switch(v, sw);
                b.trigger(slot, 0.5);
                let db = 20.0 * (rms(&mut a) / rms(&mut b)).log10();
                // Noise and the free-running oscillators differ between the
                // two runs; the clap's few bursts of filtered noise vary most.
                let tol = if slot == Slot::Cp && sw == 0 {
                    2.5
                } else {
                    1.0
                };
                assert!(db.abs() < tol, "{slot:?} sw {sw} {u:?}: {db} dB");
            }
        }
    }

    /// The low-rank Newton (factored nominal matrix + Woodbury) must agree
    /// with assembling and densely solving the full matrix, on every voice.
    #[test]
    fn low_rank_solver_matches_dense_reference() {
        let render = |dense: bool| {
            crate::circuit::stats::FORCE_DENSE.with(|f| f.set(dense));
            let mut e = Engine::new(SR);
            e.set_unit(Unit {
                seed: 7,
                age_years: 30.0,
                temp_c: 31.0,
            });
            let mut out = Vec::new();
            for blk in 0..(SR as usize / BLOCK) {
                if blk % 24 == 0 {
                    let k = (blk / 24) as u32;
                    for v in [3, 5, 6, 7, 8] {
                        e.set_switch(v, k % 2);
                    }
                    for s in 0..SLOTS {
                        e.trigger(
                            Slot::from_u32(s).unwrap(),
                            if k.is_multiple_of(3) { 1.0 } else { 0.4 },
                        );
                    }
                }
                e.process(BLOCK);
                out.extend_from_slice(&e.out);
            }
            crate::circuit::stats::FORCE_DENSE.with(|f| f.set(false));
            out
        };
        let (fast, dense) = (render(false), render(true));
        let diff = fast
            .iter()
            .zip(&dense)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        println!("max abs difference fast vs dense: {diff:e}");
        assert!(diff < 1e-6, "{diff}");
    }

    fn render(e: &mut Engine, secs: f64) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..(SR * secs) as usize / BLOCK {
            e.process(BLOCK);
            out.extend_from_slice(&e.out);
        }
        out
    }

    /// Switching the oversampling rebuilds every voice for the new rate but
    /// keeps every user setting: switching to 1x and back leaves a machine
    /// that sounds exactly like one set up from scratch.
    #[test]
    fn oversample_switch_keeps_every_setting() {
        let setup = |e: &mut Engine| {
            e.set_unit(Unit {
                seed: 11,
                age_years: 20.0,
                temp_c: 35.0,
            });
            for (v, id, x) in [
                (0, 1, 0.3),
                (0, 3, 0.9),
                (1, 0, 0.5),
                (1, 2, 0.2),
                (2, 1, 0.7),
                (3, 1, 0.9),
                (4, 1, 0.1),
                (5, 1, 0.2),
                (7, 2, 0.8),
                (9, 2, 0.9),
                (9, 1, 0.3),
            ] {
                e.set_knob(v, id, x);
            }
            e.set_part_scale(0, 0, 1.3);
            e.set_part_scale(6, 2, 0.8);
            e.set_part_scale(COMMON, 0, 3.0);
            e.set_switch(5, 1);
            e.set_switch(8, 1);
            e.set_accent(0.4);
            e.master = 1.1;
        };
        let mut a = Engine::new(SR);
        setup(&mut a);
        for s in 0..SLOTS {
            a.trigger(Slot::from_u32(s).unwrap(), 1.0);
        }
        render(&mut a, 0.2);
        a.set_oversample(1);
        assert_eq!(a.oversample(), 1);
        // Notes ringing at the switch are cut behind a fade, not a click.
        let cut = render(&mut a, 1.0);
        let step = cut.windows(2).fold(0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(step < 0.05, "{step}");
        assert!(!a.any_active());
        for f in [0, 3, 4] {
            a.set_oversample(f);
            assert_eq!(a.oversample(), 1);
        }
        a.set_oversample(2);
        let mut b = Engine::new(SR);
        setup(&mut b);
        for v in 0..=VOICES {
            for (p, q) in a.parts(v).unwrap().iter().zip(b.parts(v).unwrap()) {
                assert_eq!(p.effective, q.effective, "{v} {}", p.name);
                assert_eq!(p.user_scale, q.user_scale, "{v} {}", p.name);
            }
        }
        // The metal bank's phases run on, so compare the hi-hat and cymbal
        // by level and everything else sample for sample.
        let metal = [Slot::Ch, Slot::Oh, Slot::Cy];
        for e in [&mut a, &mut b] {
            for s in 0..SLOTS {
                let s = Slot::from_u32(s).unwrap();
                if !metal.contains(&s) {
                    e.trigger(s, 0.6);
                }
            }
        }
        let (x, y) = (render(&mut a, 0.5), render(&mut b, 0.5));
        let diff = x
            .iter()
            .zip(&y)
            .fold(0f32, |m, (p, q)| m.max((p - q).abs()));
        assert!(diff < 1e-6, "{diff}");
        render(&mut a, 4.0);
        render(&mut b, 4.0);
        for e in [&mut a, &mut b] {
            for s in metal {
                e.trigger(s, 0.6);
            }
        }
        let rms = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        let (x, y) = (render(&mut a, 0.5), render(&mut b, 0.5));
        let db = 20.0 * (rms(&x) / rms(&y)).log10();
        assert!(db.abs() < 0.5, "{db}");
    }

    /// At 1x the circuits run at the output rate: pitches and decays stay
    /// those of 2x (only the top octave changes, as the trapezoidal rule
    /// warps high-frequency filters more), and noise keeps its level.
    #[test]
    fn one_x_keeps_tunings_decays_and_levels() {
        let hit = |os: u32, slot: u32, v: usize, sw: u32| {
            let mut e = Engine::new(SR);
            e.set_oversample(os);
            e.set_switch(v, sw);
            e.trigger(Slot::from_u32(slot).unwrap(), 0.5);
            render(&mut e, 1.5)
        };
        // Frequency from upward zero crossings in [t0, t1) s.
        let freq = |x: &[f32], t0: f64, t1: f64| {
            let (mut first, mut last, mut n) = (0.0, 0.0, 0);
            for i in (t0 * SR) as usize..(t1 * SR) as usize {
                let (p, q) = (x[i - 1] as f64, x[i] as f64);
                if p <= 0.0 && q > 0.0 {
                    let at = i as f64 - q / (q - p);
                    if n == 0 {
                        first = at;
                    }
                    last = at;
                    n += 1;
                }
            }
            (n - 1) as f64 * SR / (last - first)
        };
        // Time until the 10 ms RMS stays 40 dB under its peak.
        let decay = |x: &[f32]| {
            let w = (SR * 0.01) as usize;
            let e: Vec<f64> = x
                .chunks(w)
                .map(|c| c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>())
                .collect();
            let pk = e.iter().fold(0.0f64, |m, v| m.max(*v));
            let last = e.iter().rposition(|v| *v > pk * 1e-4).unwrap();
            (last + 1) as f64 * 0.01
        };
        let rms = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        for (slot, v, sw) in CASES {
            let (x2, x1) = (hit(2, slot, v, sw), hit(1, slot, v, sw));
            let db = 20.0 * (rms(&x1) / rms(&x2)).log10();
            let (d2, d1) = (decay(&x2), decay(&x1));
            println!("slot {slot} sw {sw}: {db:+.2} dB, decay {d2} {d1}");
            assert!(db.abs() < 1.0, "slot {slot} sw {sw}: {db} dB");
            assert!(
                (d1 / d2 - 1.0).abs() < 0.08,
                "slot {slot} sw {sw}: {d2} {d1}"
            );
        }
        // Pitched voices: BD, toms and congas, claves.
        for (slot, v, sw, t0, t1) in [
            (0, 0, 0, 0.1, 0.4),
            (6, 5, 0, 0.1, 0.4),
            (6, 5, 1, 0.05, 0.25),
            (8, 7, 0, 0.1, 0.3),
            (8, 7, 1, 0.05, 0.2),
            (9, 8, 1, 0.005, 0.04),
        ] {
            let f2 = freq(&hit(2, slot, v, sw), t0, t1);
            let f1 = freq(&hit(1, slot, v, sw), t0, t1);
            println!("slot {slot} sw {sw}: {f2:.2} Hz vs {f1:.2} Hz");
            assert!(
                (f1 / f2 - 1.0).abs() < 0.01,
                "slot {slot} sw {sw}: {f2} {f1}"
            );
        }
    }

    #[test]
    fn hihat_and_cymbal_share_the_metal_bank() {
        let mut e = Engine::new(SR);
        let hz = e.metal_hz();
        // Nominal 808 frequencies within tolerance.
        for (f, n) in hz.iter().zip([205.3, 304.4, 369.6, 522.7, 540.0, 800.0]) {
            assert!((f / n - 1.0).abs() < 0.1, "{f} vs {n}");
        }
        // Editing the hi-hat's oscillator R retunes the bank both voices use.
        e.set_part_scale(2, 0, 2.0);
        let hz2 = e.metal_hz();
        assert!((hz[0] / hz2[0] - 2.0).abs() < 1e-9);
        assert_eq!(hz[1..], hz2[1..]);
        // Changing the cymbal's parts does not.
        e.set_part_scale(9, 0, 2.0);
        assert_eq!(e.metal_hz(), hz2);
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

    /// Random UI-reachable settings (oversampling included) and hits must
    /// never produce non-finite output. Run with `FUZZ_CASES=500 cargo test fuzz -- --nocapture` for more.
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
            // Half the cases start at 1×; all may switch while playing.
            if case % 2 == 1 {
                e.set_oversample(1);
            }
            let mut log = Vec::new();
            for blk in 0..(sr as usize * 3 / BLOCK) {
                if r.f() < 0.05 {
                    match r.u(6) {
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
                        4 => {
                            let (v, x) = (r.u(VOICES), r.u(2) as u32);
                            e.set_switch(v, x);
                            log.push(format!("switch {v} {x}"));
                        }
                        _ => {
                            // 0 and 3 must be ignored.
                            let f = r.u(4) as u32;
                            let before = e.oversample();
                            e.set_oversample(f);
                            let want = if f == 1 || f == 2 { f as usize } else { before };
                            assert_eq!(e.oversample(), want);
                            log.push(format!("oversample {f}"));
                        }
                    }
                }
                if blk % 4 == 0 {
                    for s in 0..SLOTS {
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
