//! Rim shot / claves (one channel, panel switch).
//!
//! - Rim shot: the trigger strikes two low-Q bridged-T resonators (~490 Hz and
//!   ~1.8 kHz) whose outputs are summed passively and thinned by an RC
//!   high-pass (~700 Hz). Both ring for only a few milliseconds.
//! - Claves: one bridged-T at ~2.55 kHz with fixed regeneration into the
//!   op-amp's + input (Q ≈ 75, ~25 ms to −20 dB; self-oscillation would need
//!   the regeneration resistor ~22 % lower than nominal, well outside part
//!   tolerance and aging), through an output coupling high-pass.
//!
//! The switch routes the trigger line to one of the two circuits. Each circuit
//! has its own sleeper, so only the selected one (plus a note still ringing
//! from before a switch change) is solved. Resonators are solved as circuits;
//! values are representative, not copied from the service manual.

use crate::blocks::{bridged_t, PartMap};
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

/// Tolerance seed salt (5 belongs to the common circuitry).
const SALT: u32 = 14;

pub struct RimClaves {
    rim: Circuit,
    cl: Circuit,
    pm: PartMap,
    pulse: Pulse,
    sleep_rim: Sleeper,
    sleep_cl: Sleeper,
    trig_rim: ElemId,
    trig_cl: ElemId,
    out_rim: Node,
    out_cl: Node,
    claves: bool,
    pending: bool,
}

impl RimClaves {
    pub fn new(sr: f64) -> Self {
        let mut pm = PartMap::default();
        let mut cb = CircuitBuilder::new();
        let t = cb.node();
        let trig_rim = cb.vsource(t, GND);
        let hi = bridged_t(
            &mut cb,
            &mut pm,
            t,
            GND,
            [
                "C1 rim high res",
                "C2 rim high res",
                "R1 rim high tune",
                "R2 rim high bridge",
                "C3 rim high strike",
                "R3 rim high strike",
            ],
            13e-9,
            100.0,
            470e3,
            100e-12,
            47e3,
        );
        let lo = bridged_t(
            &mut cb,
            &mut pm,
            t,
            GND,
            [
                "C4 rim low res",
                "C5 rim low res",
                "R4 rim low tune",
                "R5 rim low bridge",
                "C6 rim low strike",
                "R6 rim low strike",
            ],
            22e-9,
            390.0,
            560e3,
            100e-12,
            47e3,
        );
        let s = cb.node();
        let out_rim = cb.node();
        pm.r(&mut cb, hi, s, "R7 rim mix high", 10e3);
        pm.r(&mut cb, lo, s, "R8 rim mix low", 15e3);
        pm.c(&mut cb, s, out_rim, "C7 rim HPF", K::FilmCap, 10e-9);
        pm.r(&mut cb, out_rim, GND, "R9 rim HPF", 15e3);

        // Claves: a moderate-ratio bridge with fixed regeneration into the +
        // input. A bridge alone would need Rf/Rb ≈ 40 000 for this Q, beyond
        // what the op-amp's open-loop gain can realize at 2.5 kHz.
        pm.target = 1;
        let mut cb2 = CircuitBuilder::new();
        let t2 = cb2.node();
        let pos = cb2.node();
        let trig_cl = cb2.vsource(t2, GND);
        let cl = bridged_t(
            &mut cb2,
            &mut pm,
            t2,
            pos,
            [
                "C8 claves res",
                "C9 claves res",
                "R10 claves tune",
                "R11 claves bridge",
                "C10 claves strike",
                "R12 claves strike",
            ],
            3.9e-9,
            390.0,
            680e3,
            22e-12,
            47e3,
        );
        pm.r(&mut cb2, cl, pos, "R13 claves regen", 130e3);
        pm.r(&mut cb2, pos, GND, "R14 claves regen ground", 100.0);
        let out_cl = cb2.node();
        pm.c(&mut cb2, cl, out_cl, "C11 claves output", K::FilmCap, 10e-9);
        pm.r(&mut cb2, out_cl, GND, "R15 claves load", 47e3);

        let (rim, cl) = (cb.build(sr), cb2.build(sr));
        let (sleep_rim, sleep_cl) = (Sleeper::new(sr, &[&rim]), Sleeper::new(sr, &[&cl]));
        let mut v = Self {
            rim,
            cl,
            pm,
            pulse: Pulse::new(sr),
            sleep_rim,
            sleep_cl,
            trig_rim,
            trig_cl,
            out_rim,
            out_cl,
            claves: false,
            pending: false,
        };
        v.refresh(&Unit::default());
        v
    }

    pub fn is_claves(&self) -> bool {
        self.claves
    }
}

impl Voice for RimClaves {
    fn parts(&self) -> &[Part] {
        &self.pm.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }
    fn set_knob(&mut self, _id: u32, _v: f64) {}
    fn set_switch(&mut self, value: u32) {
        self.pending = value == 1;
    }
    fn refresh(&mut self, unit: &Unit) {
        self.sleep_rim.restore(&mut [&mut self.rim]);
        self.sleep_cl.restore(&mut [&mut self.cl]);
        self.pm
            .refresh_all(&mut [&mut self.rim, &mut self.cl], unit, SALT);
        self.sleep_rim.resume(&[&self.rim]);
        self.sleep_cl.resume(&[&self.cl]);
    }
    fn trigger(&mut self, accent: f64, _variant: u32) {
        self.claves = self.pending;
        self.pulse.fire(accent);
        if self.claves {
            self.sleep_cl.wake(&mut [&mut self.cl]);
        } else {
            self.sleep_rim.wake(&mut [&mut self.rim]);
        }
    }
    fn tick(&mut self, bus: &Bus) -> f64 {
        let p = self.pulse.next(bus);
        let mut y = 0.0;
        if self.sleep_rim.awake() {
            self.rim.rail = bus.rail;
            self.rim
                .set_voltage(self.trig_rim, if self.claves { 0.0 } else { p });
            self.rim.step();
            y += self.sleep_rim.hear(self.rim.v(self.out_rim));
        }
        if self.sleep_cl.awake() {
            self.cl.rail = bus.rail;
            self.cl
                .set_voltage(self.trig_cl, if self.claves { p } else { 0.0 });
            self.cl.step();
            y += self.sleep_cl.hear(self.cl.v(self.out_cl));
        }
        y
    }
    fn active(&self) -> bool {
        self.sleep_rim.awake() || self.sleep_cl.awake()
    }
    fn housekeep(&mut self) {
        let idle = !self.pulse.busy();
        self.sleep_rim.housekeep(&[&self.rim], idle);
        self.sleep_cl.housekeep(&[&self.cl], idle);
    }
    fn set_quiet(&mut self, volts: f64) {
        self.sleep_rim.set_threshold(volts);
        self.sleep_cl.set_threshold(volts);
    }
    fn accent_draw(&self) -> f64 {
        self.pulse.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.active(), &[&self.rim, &self.cl])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peak op-amp swing of the selected circuit after one hit.
    fn opamp_peak(claves: bool, accent: f64) -> f64 {
        let sr = 96_000.0;
        let mut v = RimClaves::new(sr);
        v.set_switch(claves as u32);
        v.trigger(accent, 0);
        let mut peak = 0.0f64;
        for i in 0..(sr as usize / 10) {
            v.tick(&Bus::default());
            if i < 150 {
                continue;
            }
            let c = if claves { &v.cl } else { &v.rim };
            // Op-amp outputs: rim high and low resonators, claves resonator.
            let outs: &[usize] = if claves { &[6] } else { &[5, 9] };
            for &n in outs {
                peak = peak.max(c.v(n).abs());
            }
        }
        peak
    }

    #[test]
    fn resonators_ring_below_the_rails() {
        // Past the trigger edge, a normal hit stays linear; full accent may
        // touch the rails only on the rim shot, which is meant to be hard.
        assert!(opamp_peak(false, 0.5) < 7.0);
        assert!(opamp_peak(true, 0.5) < 5.0);
        assert!(opamp_peak(true, 1.0) < 7.0);
    }

    #[test]
    fn claves_never_self_oscillate() {
        let sr = 48_000.0;
        for seed in 0..40 {
            for (age_years, temp_c) in [(0.0, 25.0), (45.0, 0.0), (45.0, 45.0), (0.0, 45.0)] {
                let mut v = RimClaves::new(sr);
                v.refresh(&Unit {
                    seed,
                    age_years,
                    temp_c,
                });
                v.set_switch(1);
                v.trigger(1.0, 0);
                for i in 0..(sr * 0.4) as usize {
                    if i % crate::voice::book_every(sr) == 0 {
                        v.housekeep();
                    }
                    v.tick(&Bus::default());
                }
                assert!(!v.active(), "seed {seed} age {age_years} temp {temp_c}");
            }
        }
    }

    #[test]
    fn switch_picks_the_circuit_for_the_next_trigger() {
        let mut v = RimClaves::new(48_000.0);
        v.trigger(0.5, 0);
        v.set_switch(1);
        v.tick(&Bus::default());
        assert!(v.sleep_rim.awake() && !v.sleep_cl.awake());
        v.trigger(0.5, 0);
        // The rim shot may finish; the claves now ring too.
        assert!(v.sleep_rim.awake() && v.sleep_cl.awake());
    }
}
