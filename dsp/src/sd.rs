//! Snare: two bridged-T resonators (body) excited by the trigger, plus a
//! white-noise branch through a band-pass RC network and a VCA driven by a
//! peak-detector envelope ("snappy").
//!
//! The resonators, filters and envelope are solved as a circuit. The noise
//! source and the VCA gain law are behavioral.

use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{apply_unit, Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

const VOICE_ID: u32 = 1;

pub struct Snare {
    c: Circuit,
    parts: Vec<Part>,
    pulse: Pulse,
    sleep: Sleeper,
    trig: ElemId,
    noise_src: ElemId,
    /// (element, part index) pairs refreshed from part values.
    map: Vec<(ElemId, usize, bool)>,
    diodes: Vec<(ElemId, usize)>,
    out_a: Node,
    out_b: Node,
    noise_out: Node,
    env: Node,
    knob_tone: f64,
    knob_snappy: f64,
    rng: u32,
}

impl Snare {
    pub fn new(sr: f64) -> Self {
        let mut cb = CircuitBuilder::new();
        let mut parts: Vec<Part> = Vec::new();
        let mut map: Vec<(ElemId, usize, bool)> = Vec::new();
        let mut diodes = Vec::new();
        let part = |parts: &mut Vec<Part>, name, kind, v| {
            parts.push(Part::new(name, kind, v));
            parts.len() - 1
        };

        let n_trig = cb.node();
        let trig = cb.vsource(n_trig, GND);

        // Two resonators (low ~180 Hz, high ~330 Hz).
        let mut outs = [GND; 2];
        let specs: [(&'static [&'static str; 6], f64, f64, f64); 2] = [
            (
                &[
                    "C1 low res",
                    "C2 low res",
                    "R1 low tune",
                    "R2 low decay",
                    "C3 low drive",
                    "R3 low drive",
                ],
                47e-9,
                277.0,
                1.28e6,
            ),
            (
                &[
                    "C4 high res",
                    "C5 high res",
                    "R4 high tune",
                    "R5 high decay",
                    "C6 high drive",
                    "R6 high drive",
                ],
                33e-9,
                178.0,
                1.2e6,
            ),
        ];
        for (k, (names, cap, rb, rf)) in specs.iter().enumerate() {
            let ka = cb.node();
            let inv = cb.node();
            let mid = cb.node();
            let out = cb.node();
            let e_cd = cb.capacitor(n_trig, ka, 1.0);
            let e_rd = cb.resistor(ka, inv, 1.0);
            let e_c1 = cb.capacitor(out, mid, 1.0);
            let e_c2 = cb.capacitor(mid, inv, 1.0);
            let e_rb = cb.resistor(mid, GND, 1.0);
            let e_rf = cb.resistor(out, inv, 1.0);
            cb.opamp(GND, inv, out);
            map.push((e_c1, part(&mut parts, names[0], K::FilmCap, *cap), true));
            map.push((e_c2, part(&mut parts, names[1], K::FilmCap, *cap), true));
            map.push((e_rb, part(&mut parts, names[2], K::Carbon, *rb), false));
            map.push((e_rf, part(&mut parts, names[3], K::Carbon, *rf), false));
            map.push((e_cd, part(&mut parts, names[4], K::Ceramic, 10e-9), true));
            map.push((e_rd, part(&mut parts, names[5], K::Carbon, 47e3), false));
            outs[k] = out;
        }

        // Noise branch: high-pass then low-pass.
        let n_noise = cb.node();
        let n_hp = cb.node();
        let n_lp = cb.node();
        let noise_src = cb.vsource(n_noise, GND);
        let e = cb.capacitor(n_noise, n_hp, 1.0);
        map.push((
            e,
            part(&mut parts, "C7 noise HPF", K::FilmCap, 4.7e-9),
            true,
        ));
        let e = cb.resistor(n_hp, GND, 1.0);
        map.push((e, part(&mut parts, "R7 noise HPF", K::Carbon, 10e3), false));
        let e = cb.resistor(n_hp, n_lp, 1.0);
        map.push((e, part(&mut parts, "R8 noise LPF", K::Carbon, 4.7e3), false));
        let e = cb.capacitor(n_lp, GND, 1.0);
        map.push((
            e,
            part(&mut parts, "C8 noise LPF", K::Ceramic, 2.2e-9),
            true,
        ));

        // Snappy envelope: peak detector.
        let n_chg = cb.node();
        let n_env = cb.node();
        let e = cb.resistor(n_trig, n_chg, 1.0);
        map.push((e, part(&mut parts, "R9 env charge", K::Carbon, 1e3), false));
        let d = cb.diode(n_chg, n_env, 2.52e-9, 1.752, 0.02585);
        diodes.push((d, part(&mut parts, "D1 env detector", K::Diode, 2.52e-9)));
        let e = cb.capacitor(n_env, GND, 1.0);
        map.push((
            e,
            part(&mut parts, "C9 snappy env", K::Electrolytic, 1e-6),
            true,
        ));
        let e = cb.resistor(n_env, GND, 1.0);
        map.push((
            e,
            part(&mut parts, "R10 snappy decay", K::Carbon, 150e3),
            false,
        ));

        let mut sd = Self {
            c: cb.build(sr),
            parts,
            pulse: Pulse::new(sr),
            sleep: Sleeper::new(sr),
            trig,
            noise_src,
            map,
            diodes,
            out_a: outs[0],
            out_b: outs[1],
            noise_out: n_lp,
            env: n_env,
            knob_tone: 0.5,
            knob_snappy: 0.6,
            rng: 0x1234_5678,
        };
        sd.refresh(&Unit::default());
        sd
    }

    fn noise(&mut self) -> f64 {
        // xorshift32 white noise in [-1, 1).
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f64 / u32::MAX as f64) * 2.0 - 1.0
    }
}

impl Voice for Snare {
    fn parts(&self) -> &[Part] {
        &self.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.parts
    }

    fn set_knob(&mut self, id: u32, v: f64) {
        match id {
            1 => self.knob_tone = v,
            2 => self.knob_snappy = v,
            _ => {}
        }
    }

    fn refresh(&mut self, unit: &Unit) {
        apply_unit(&mut self.parts, unit, VOICE_ID);
        for &(id, pi, is_cap) in &self.map {
            let v = self.parts[pi].effective;
            if is_cap {
                self.c.set_capacitance(id, v);
            } else {
                self.c.set_resistance(id, v);
            }
        }
        let vt = unit.vt();
        for &(id, pi) in &self.diodes {
            let p = &self.parts[pi];
            self.c.set_diode(id, p.effective, p.n, vt);
        }
    }

    fn trigger(&mut self, accent: f64, _variant: u32) {
        self.pulse.fire(accent);
        self.sleep.wake();
    }

    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake {
            return 0.0;
        }
        let v = self.pulse.next(bus);
        self.c.rail = bus.rail;
        let n = self.noise() * bus.noise;
        self.c.set_voltage(self.trig, v);
        // Gate the noise generator with the envelope so the voice can sleep.
        let gate = self.c.v(self.env).clamp(0.0, 1.0);
        self.c.set_voltage(self.noise_src, n * 2.0 * gate);
        self.c.step();
        self.sleep.observe(&[&self.c]);
        let body = (1.0 - self.knob_tone) * self.c.v(self.out_a)
            + (0.4 + self.knob_tone) * self.c.v(self.out_b);
        // Behavioral VCA: exponential-ish gain from the envelope voltage.
        let env = (self.c.v(self.env) - 0.2).max(0.0) / 8.0;
        let vca = env * env * 6.0;
        body * 0.6 + self.c.v(self.noise_out) * vca * self.knob_snappy * 3.0
    }

    fn active(&self) -> bool {
        self.sleep.awake
    }

    fn accent_draw(&self) -> f64 {
        self.pulse.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.sleep.awake, &[&self.c])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_plan_matches_dense_solver() {
        let sr = 96_000.0;
        let mut a = Snare::new(sr);
        let mut b = Snare::new(sr);
        b.c.dense_only = true;
        let mut max_diff = 0.0f64;
        for i in 0..(sr as usize / 2) {
            if i % 12_000 == 0 {
                a.trigger(0.8, 0);
                b.trigger(0.8, 0);
            }
            let bus = Bus::default();
            max_diff = max_diff.max((a.tick(&bus) - b.tick(&bus)).abs());
        }
        assert!(max_diff < 1e-6, "{max_diff}");
    }

    #[test]
    fn onset_cost() {
        let sr = 96_000.0;
        let mut sd = Snare::new(sr);
        let bus = Bus::default();
        for _ in 0..1000 {
            sd.tick(&bus);
        }
        let before = sd.c.replans;
        sd.trigger(0.35, 0);
        let mut its = Vec::new();
        for _ in 0..256 {
            sd.tick(&bus);
            its.push(sd.c.last_iters);
        }
        // Recovering from op-amp saturation must not walk the iteration cap.
        assert!(its.iter().all(|&n| n < 40), "{its:?}");
        assert!(its.iter().sum::<usize>() < 1500);
        assert!(sd.c.replans - before <= 4);
    }
}
