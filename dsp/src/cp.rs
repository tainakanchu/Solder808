//! Hand clap: noise → ~1.1 kHz band-pass → VCA. The VCA is driven by two
//! envelopes: a "clap" detector re-triggered by a short burst of pulses with a
//! fast discharge, and a slower "tail" detector that gives the reverb-like
//! decay. Noise and the VCA law are behavioral; filters and envelopes are
//! solved as a circuit.
//!
//! Maracas share the channel (CP/MA switch, clap level): white noise under a
//! VCA with a short peak-detector envelope, then a two-pole RC high-pass
//! (~5 kHz). It is a second, smaller circuit with its own sleeper; the switch
//! picks which one the next trigger strikes, and a note already ringing in the
//! other finishes. Its parts follow the clap's in the part table.

use crate::blocks::{mfb_bandpass, peak_det, peak_env, vca_gain, Noise, PartMap};
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

const VOICE_ID: u32 = 3;
/// Burst pulse start times (ms) — the 808-style triple slap before the tail.
const BURST_MS: [f64; 4] = [0.0, 10.0, 20.0, 30.0];
const VR_TAIL_MAX: f64 = 470e3;
/// Maracas noise drive (V) at full VCA gain.
const MA_DRIVE: f64 = 4.0;

pub struct Clap {
    c: Circuit,
    pm: PartMap,
    sleep: Sleeper,
    noise: Noise,
    sr: f64,
    src: ElemId,
    trig_burst: ElemId,
    trig_tail: ElemId,
    env_burst: Node,
    env_tail: Node,
    out: Node,
    /// Samples since trigger, or u32::MAX when idle.
    t: u32,
    /// Accent amount of the current hit (0..1).
    level: f64,
    /// True while a burst or tail pulse is high (loads the accent line).
    pulsing: bool,
    width: u32,
    vr_tail: usize,
    r_tail: ElemId,
    knob_tail: f64,
    ma: Circuit,
    ma_sleep: Sleeper,
    ma_pulse: Pulse,
    ma_noise: Noise,
    ma_trig: ElemId,
    ma_src: ElemId,
    ma_env: Node,
    ma_out: Node,
    maracas: bool,
    pending: bool,
}

impl Clap {
    pub fn new(sr: f64) -> Self {
        let mut cb = CircuitBuilder::new();
        let mut pm = PartMap::default();
        let n_src = cb.node();
        let src = cb.vsource(n_src, GND);
        let bp = mfb_bandpass(
            &mut cb,
            &mut pm,
            n_src,
            ["R1 BPF in", "C1 BPF", "C2 BPF", "R2 BPF fb"],
            2.9e3,
            10e-9,
            72e3,
        );
        let (trig_burst, env_burst) = peak_env(
            &mut cb,
            &mut pm,
            ["R3 burst charge", "D1 burst", "C3 burst", "R4 burst decay"],
            0.22e-6,
            15e3,
        );
        // Tail: the discharge resistor is a fixed part plus the tail pot.
        let n_t = cb.node();
        let n_k = cb.node();
        let env_tail = cb.node();
        let trig_tail = cb.vsource(n_t, GND);
        pm.r(&mut cb, n_t, n_k, "R5 tail charge", 1e3);
        pm.d(&mut cb, n_k, env_tail, "D2 tail");
        pm.c(&mut cb, env_tail, GND, "C4 tail", K::Electrolytic, 1e-6);
        let r_tail = cb.resistor(env_tail, GND, 1.0);
        pm.add("R6 tail (fixed)", K::Carbon, 33e3);
        let vr_tail = pm.add("VR1 tail", K::Pot, VR_TAIL_MAX);
        // Output coupling.
        let out = cb.node();
        pm.c(&mut cb, bp, out, "C5 output", K::Electrolytic, 1e-6);
        pm.r(&mut cb, out, GND, "R7 load", 10e3);

        // Maracas.
        pm.target = 1;
        let mut mb = CircuitBuilder::new();
        let mt = mb.node();
        let ma_trig = mb.vsource(mt, GND);
        let ma_env = peak_det(
            &mut mb,
            &mut pm,
            mt,
            ["R8 MA charge", "D3 MA env", "C6 MA env", "R9 MA decay"],
            1e3,
            0.47e-6,
            47e3,
        );
        let n0 = mb.node();
        let h1 = mb.node();
        let ma_out = mb.node();
        let ma_src = mb.vsource(n0, GND);
        pm.c(&mut mb, n0, h1, "C7 MA HPF", K::FilmCap, 2.2e-9);
        pm.r(&mut mb, h1, GND, "R10 MA HPF", 15e3);
        pm.c(&mut mb, h1, ma_out, "C8 MA HPF", K::FilmCap, 2.2e-9);
        pm.r(&mut mb, ma_out, GND, "R11 MA HPF", 15e3);

        let mut cp = Self {
            ma: mb.build(sr),
            ma_sleep: Sleeper::new(sr),
            ma_pulse: Pulse::new(sr),
            ma_noise: Noise(0x5851_f42d),
            ma_trig,
            ma_src,
            ma_env,
            ma_out,
            maracas: false,
            pending: false,
            c: cb.build(sr),
            pm,
            sleep: Sleeper::new(sr),
            noise: Noise(0x9e37_79b9),
            sr,
            src,
            trig_burst,
            trig_tail,
            env_burst,
            env_tail,
            out,
            t: u32::MAX,
            level: 0.0,
            pulsing: false,
            width: (sr * 1e-3) as u32,
            vr_tail,
            r_tail,
            knob_tail: 0.4,
        };
        cp.refresh(&Unit::default());
        cp
    }
}

impl Voice for Clap {
    fn parts(&self) -> &[Part] {
        &self.pm.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }
    fn set_knob(&mut self, id: u32, v: f64) {
        if id == 1 {
            self.knob_tail = v;
        }
    }
    fn set_switch(&mut self, value: u32) {
        self.pending = value == 1;
    }
    fn refresh(&mut self, unit: &Unit) {
        self.pm.parts[self.vr_tail].nominal = VR_TAIL_MAX * self.knob_tail;
        self.pm
            .refresh_all(&mut [&mut self.c, &mut self.ma], unit, VOICE_ID);
        let r = self.pm.parts[self.vr_tail - 1].effective + self.pm.parts[self.vr_tail].effective;
        self.c.set_resistance(self.r_tail, r);
    }
    fn trigger(&mut self, accent: f64, _variant: u32) {
        self.maracas = self.pending;
        if self.maracas {
            self.ma_pulse.fire(accent);
            self.ma_sleep.wake();
        } else {
            self.t = 0;
            self.level = accent.clamp(0.0, 1.0);
            self.sleep.wake();
        }
    }
    fn tick(&mut self, bus: &Bus) -> f64 {
        let mut y = 0.0;
        if self.ma_sleep.awake {
            y += self.tick_maracas(bus);
        }
        if self.sleep.awake {
            y += self.tick_clap(bus);
        }
        y
    }
    fn active(&self) -> bool {
        self.sleep.awake || self.ma_sleep.awake
    }
    fn accent_draw(&self) -> f64 {
        self.ma_pulse.draw() + if self.pulsing { self.level } else { 0.0 }
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.active(), &[&self.c, &self.ma])
    }
}

impl Clap {
    fn tick_maracas(&mut self, bus: &Bus) -> f64 {
        let p = self.ma_pulse.next(bus);
        let g = vca_gain(self.ma.v(self.ma_env));
        let n = self.ma_noise.tick();
        self.ma.rail = bus.rail;
        self.ma.set_voltage(self.ma_trig, p);
        // VCA ahead of the linear high-pass, as on the clap.
        self.ma.set_voltage(self.ma_src, n * g * MA_DRIVE);
        self.ma.step();
        self.ma_sleep.observe(&[&self.ma]);
        self.ma.v(self.ma_out)
    }

    fn tick_clap(&mut self, bus: &Bus) -> f64 {
        let mut burst = 0.0;
        let mut tail = 0.0;
        let v = (4.0 + 8.0 * self.level * bus.accent) * bus.rail;
        if self.t != u32::MAX {
            let ms = self.t as f64 * 1e3 / self.sr;
            if BURST_MS
                .iter()
                .any(|&s| ms >= s && ms < s + self.width as f64 * 1e3 / self.sr)
            {
                burst = v;
            }
            // The tail starts with the last burst pulse.
            let last = BURST_MS[BURST_MS.len() - 1];
            if ms >= last && ms < last + 1.0 {
                tail = v * 0.7;
            }
            self.t = if ms > 40.0 { u32::MAX } else { self.t + 1 };
        }
        let g = vca_gain(self.c.v(self.env_burst)) * 6.0 + vca_gain(self.c.v(self.env_tail)) * 3.0;
        // VCA ahead of the filter keeps the noise silent between hits.
        let n = self.noise.tick();
        self.c.set_voltage(self.src, n * g * 0.25);
        self.c.set_voltage(self.trig_burst, burst);
        self.c.set_voltage(self.trig_tail, tail);
        self.pulsing = burst > 0.0 || tail > 0.0;
        self.c.rail = bus.rail;
        self.c.step();
        self.sleep.observe(&[&self.c]);
        self.c.v(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newton_iterations_when_retriggered() {
        let sr = 96_000.0;
        let mut cp = Clap::new(sr);
        let step = (sr * 60.0 / 140.0 / 4.0) as usize;
        let mut hist = [0usize; 41];
        for i in 0..(sr as usize * 2) {
            if i % step == 0 {
                cp.trigger(0.5, 0);
            }
            cp.tick(&Bus::default());
            hist[cp.c.last_iters.min(40)] += 1;
        }
        // Newton must converge without hitting the iteration cap.
        assert_eq!(hist[40], 0, "{hist:?}");
    }
}
