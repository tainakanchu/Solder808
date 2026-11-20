//! Tom / conga channel (LT/LC, MT/MC, HT/HC). One circuit per channel, with
//! the panel's Tom/Conga switch selecting components the way a multi-pole
//! switch would:
//!
//! - Body: a bridged-T resonator around an inverting op-amp, like the BD's but
//!   tuned higher and without the BD's regeneration: the toms have no decay
//!   control, so the bridge alone sets Q.
//!   The bridge's ground leg Rb is the tuning resistor plus trimmer in series
//!   with a forward-biased diode. Since f ∝ 1/(C√(Rb·Rf)) while Q ∝ √(Rf/Rb),
//!   the ring time τ = 2Q/ω = C·Rf does not move with the tuning knob. The
//!   switch swaps the resonator's two capacitors (tom pair / conga pair) and
//!   which trimmer sits in the ground leg, so each mode keeps its own tuning
//!   knob; the smaller conga capacitors also shorten τ.
//! - Pitch drop: the diode in the ground leg carries a standing bias current
//!   from the supply, so at rest it is a small resistance nVt/I. A peak
//!   detector charged by the trigger adds current through a bias network;
//!   the diode's resistance falls, Rb falls and the pitch jumps, then glides
//!   back down as the envelope discharges (both modes). The bias network is
//!   high impedance next to the diode, so it does not load the resonator.
//!   The same current step is the strike: the step at the ground leg rings
//!   the bridge. An RC in the bias path rounds it to ~1.5 ms, which sets the
//!   attack and keeps the op-amp below its rails (an unrounded step, or a
//!   capacitive strike into the inverting input of a bridge this high in Q,
//!   clips it). Accent therefore deepens the bend as well as the level. The
//!   rest pitch follows the supply rail and temperature slightly, as a
//!   diode-biased circuit would.
//! - Tom only: a "pink-ish" noise burst — white noise through a lag–lead shelf
//!   (−6 dB/oct between ~0.6 and ~3 kHz), a coupling high-pass and an RC
//!   low-pass — under its own VCA, whose short peak-detector envelope is
//!   charged from the trigger line through a switch contact that the conga
//!   position opens.
//!
//! The resonator, envelopes and noise filter are solved as circuits; the noise
//! source and VCA gain law are behavioral, as in the other voices. The noise
//! filter is its own small linear circuit, stepped only while its VCA is open
//! or it still holds energy, so a conga (or a decayed tom) never pays for it.
//! Values are representative of the 808's signal flow, not copied from the
//! service manual.

use crate::blocks::{peak_det, vca_gain, Noise, PartMap};
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

/// Component values that differ between the three channels.
pub struct TomSpec {
    /// Tolerance seed salt for this channel's part table.
    pub salt: u32,
    /// Resonator capacitors (each of the pair) in tom / conga position.
    pub c_tom: f64,
    pub c_conga: f64,
    /// Fixed tuning resistor in the bridge's ground leg.
    pub r_tune: f64,
    /// Full travel of the tom / conga tuning trimmers.
    pub vr_tom: f64,
    pub vr_conga: f64,
    /// Bridge resistor Rf (output → inverting input); with C sets the decay.
    pub r_bridge: f64,
    pub noise_seed: u32,
}

/// Seed salts 11..13: 5 is the common circuitry's, kept so existing units do
/// not change.
pub const LOW: TomSpec = TomSpec {
    salt: 11,
    c_tom: 75e-9,
    c_conga: 36e-9,
    r_tune: 47.0,
    vr_tom: 150.0,
    vr_conga: 150.0,
    r_bridge: 2.2e6,
    noise_seed: 0x2545_f491,
};
pub const MID: TomSpec = TomSpec {
    salt: 12,
    c_tom: 39e-9,
    c_conga: 20e-9,
    r_tune: 47.0,
    vr_tom: 150.0,
    vr_conga: 150.0,
    r_bridge: 3.3e6,
    noise_seed: 0x6c07_8965,
};
pub const HIGH: TomSpec = TomSpec {
    salt: 13,
    c_tom: 22e-9,
    c_conga: 12e-9,
    r_tune: 47.0,
    vr_tom: 150.0,
    vr_conga: 150.0,
    r_bridge: 5.6e6,
    noise_seed: 0x9908_b0df,
};

const RAIL: f64 = 15.0;
/// Noise generator drive (V) into the shelf filter at full VCA gain.
const NOISE_DRIVE: f64 = 6.0;
/// Behavioral mix of the noise VCA output into the channel output.
const NOISE_MIX: f64 = 6.0;

pub struct TomConga {
    c: Circuit,
    nz: Circuit,
    pm: PartMap,
    pulse: Pulse,
    sleep: Sleeper,
    noise: Noise,
    salt: u32,
    trig: ElemId,
    ntrig: ElemId,
    nsrc: ElemId,
    e_c1: ElemId,
    e_c2: ElemId,
    e_rb: ElemId,
    e_rf: ElemId,
    e_nch: ElemId,
    e_stand: ElemId,
    p_c1t: usize,
    p_c2t: usize,
    p_c1c: usize,
    p_c2c: usize,
    p_rt: usize,
    p_vrt: usize,
    p_vrc: usize,
    p_rf: usize,
    p_nch: usize,
    p_stand: usize,
    vr_tom: f64,
    vr_conga: f64,
    /// Nodes watched by the sleeper: ones that rest at exactly 0 V (the diode
    /// bias holds the ground leg, the bridge middle and the pitch envelope at
    /// a standing DC level, so they never go quiet).
    watch: [Node; 3],
    /// Noise filter nodes (to tell when the burst has died away).
    filt: [Node; 3],
    out: Node,
    nenv: Node,
    nout: Node,
    /// Noise circuit still being stepped.
    nz_live: bool,
    conga: bool,
    pending: bool,
    knob_tom: f64,
    knob_conga: f64,
}

impl TomConga {
    pub fn new(sr: f64, spec: &TomSpec) -> Self {
        let mut pm = PartMap::default();
        let mut cb = CircuitBuilder::new();
        let trig_n = cb.node();
        let inv = cb.node();
        let mid = cb.node();
        let o = cb.node();
        let x = cb.node();
        let trig = cb.vsource(trig_n, GND);

        // Bridged-T resonator; the switched elements are pushed by `apply_mode`.
        let e_c1 = cb.capacitor(o, mid, spec.c_tom);
        let e_c2 = cb.capacitor(mid, inv, spec.c_tom);
        let e_rb = cb.resistor(mid, x, spec.r_tune);
        let e_rf = cb.resistor(o, inv, spec.r_bridge);
        let p_c1t = pm.add("C1 tom resonator", K::FilmCap, spec.c_tom);
        let p_c2t = pm.add("C2 tom resonator", K::FilmCap, spec.c_tom);
        let p_c1c = pm.add("C3 conga resonator", K::FilmCap, spec.c_conga);
        let p_c2c = pm.add("C4 conga resonator", K::FilmCap, spec.c_conga);
        let p_rt = pm.add("R1 tune", K::Carbon, spec.r_tune);
        let p_vrt = pm.add("VR1 tom tune", K::Pot, spec.vr_tom);
        let p_vrc = pm.add("VR2 conga tune", K::Pot, spec.vr_conga);
        let p_rf = pm.add("R2 bridge", K::Carbon, spec.r_bridge);
        cb.opamp(GND, inv, o);
        // Ground-leg diode with its standing bias (supply through a resistor,
        // as its Norton equivalent) and the pitch envelope's bias resistor.
        pm.d(&mut cb, x, GND, "D1 pitch");
        let e_stand = cb.isource(GND, x);
        let p_stand = pm.r(&mut cb, x, GND, "R3 pitch standing bias", 47e3);
        let env = peak_det(
            &mut cb,
            &mut pm,
            trig_n,
            [
                "R4 pitch env charge",
                "D2 pitch env",
                "C5 pitch env",
                "R5 pitch env discharge",
            ],
            220.0,
            4.7e-6,
            100e3,
        );
        // Bias path with an RC that rounds off the current step: a step at
        // the ground leg would strike the bridge far harder than the trigger.
        let b = cb.node();
        pm.r(&mut cb, env, b, "R6 pitch bias", 3.3e3);
        pm.c(
            &mut cb,
            b,
            GND,
            "C6 pitch smoothing",
            K::Electrolytic,
            0.47e-6,
        );
        pm.r(&mut cb, b, x, "R7 pitch bias", 3.3e3);
        // Output coupling.
        let out = cb.node();
        pm.c(&mut cb, o, out, "C7 output", K::Electrolytic, 1e-6);
        pm.r(&mut cb, out, GND, "R8 load", 10e3);

        // Noise envelope and filter: a separate circuit, joined to the body
        // only through the (ideal) trigger line, so splitting it off is exact.
        pm.target = 1;
        let mut nb = CircuitBuilder::new();
        let nt = nb.node();
        let ntrig = nb.vsource(nt, GND);
        // The envelope's charge path is the switch contact.
        let nk = nb.node();
        let nenv = nb.node();
        let e_nch = nb.resistor(nt, nk, 1e3);
        let p_nch = pm.add("R9 noise env charge", K::Carbon, 1e3);
        pm.d(&mut nb, nk, nenv, "D3 noise env");
        pm.c(&mut nb, nenv, GND, "C8 noise env", K::Electrolytic, 0.47e-6);
        pm.r(&mut nb, nenv, GND, "R10 noise env discharge", 68e3);
        let n0 = nb.node();
        let n1 = nb.node();
        let n2 = nb.node();
        let nout = nb.node();
        let nsrc = nb.vsource(n0, GND);
        pm.r(&mut nb, n0, n1, "R11 noise shelf", 10e3);
        pm.c(&mut nb, n1, n2, "C9 noise shelf", K::FilmCap, 22e-9);
        pm.r(&mut nb, n2, GND, "R12 noise shelf", 2.2e3);
        pm.c(&mut nb, n1, nout, "C10 noise HPF", K::FilmCap, 0.1e-6);
        pm.r(&mut nb, nout, GND, "R13 noise HPF", 10e3);
        pm.c(&mut nb, nout, GND, "C11 noise LPF", K::Ceramic, 22e-9);

        let mut t = Self {
            c: cb.build(sr),
            nz: nb.build(sr),
            pm,
            pulse: Pulse::new(sr),
            sleep: Sleeper::new(sr),
            noise: Noise(spec.noise_seed),
            salt: spec.salt,
            trig,
            ntrig,
            nsrc,
            e_c1,
            e_c2,
            e_rb,
            e_rf,
            e_nch,
            e_stand,
            p_c1t,
            p_c2t,
            p_c1c,
            p_c2c,
            p_rt,
            p_vrt,
            p_vrc,
            p_rf,
            p_nch,
            p_stand,
            vr_tom: spec.vr_tom,
            vr_conga: spec.vr_conga,
            watch: [o, out, inv],
            filt: [n1, n2, nout],
            out,
            nenv,
            nout,
            nz_live: false,
            conga: false,
            pending: false,
            knob_tom: 0.5,
            knob_conga: 0.5,
        };
        t.refresh(&Unit::default());
        t.power_on(sr);
        t
    }

    /// Let the diode's standing bias charge the bridge capacitors before the
    /// first note, with the bridge heavily damped so the step does not ring.
    fn power_on(&mut self, sr: f64) {
        self.c.set_resistance(self.e_rf, 1e3);
        self.set_bias(1.0);
        for _ in 0..(sr * 0.02) as usize {
            self.c.step();
        }
        self.c.set_resistance(self.e_rf, self.pm.eff(self.p_rf));
    }

    fn set_bias(&mut self, rail: f64) {
        let r = self.pm.eff(self.p_stand);
        self.c.set_current(self.e_stand, RAIL * rail / r);
    }

    /// Push the switched elements for the current mode.
    fn apply_mode(&mut self) {
        let pm = &self.pm;
        let (c1, c2, vr) = if self.conga {
            (self.p_c1c, self.p_c2c, self.p_vrc)
        } else {
            (self.p_c1t, self.p_c2t, self.p_vrt)
        };
        self.c.set_capacitance(self.e_c1, pm.eff(c1));
        self.c.set_capacitance(self.e_c2, pm.eff(c2));
        self.c
            .set_resistance(self.e_rb, pm.eff(self.p_rt) + pm.eff(vr));
        let nch = if self.conga { 1e9 } else { pm.eff(self.p_nch) };
        self.nz.set_resistance(self.e_nch, nch);
    }

    pub fn is_conga(&self) -> bool {
        self.conga
    }
}

impl Voice for TomConga {
    fn parts(&self) -> &[Part] {
        &self.pm.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }
    fn set_knob(&mut self, id: u32, v: f64) {
        match id {
            1 => self.knob_tom = v,
            2 => self.knob_conga = v,
            _ => {}
        }
    }
    fn set_switch(&mut self, value: u32) {
        self.pending = value == 1;
    }
    fn refresh(&mut self, unit: &Unit) {
        self.pm.parts[self.p_vrt].nominal = self.vr_tom * (1.0 - self.knob_tom);
        self.pm.parts[self.p_vrc].nominal = self.vr_conga * (1.0 - self.knob_conga);
        self.pm
            .refresh_all(&mut [&mut self.c, &mut self.nz], unit, self.salt);
        self.c.set_resistance(self.e_rf, self.pm.eff(self.p_rf));
        self.apply_mode();
    }
    fn trigger(&mut self, accent: f64, _variant: u32) {
        if self.pending != self.conga {
            self.conga = self.pending;
            self.apply_mode();
        }
        self.pulse.fire(accent);
        self.sleep.wake();
    }
    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake {
            return 0.0;
        }
        let p = self.pulse.next(bus);
        self.c.rail = bus.rail;
        self.c.set_voltage(self.trig, p);
        self.set_bias(bus.rail);
        self.c.step();
        // The noise circuit runs from a tom trigger until its envelope has
        // fallen well below the VCA's threshold and the filter is quiet; a
        // conga never charges the envelope, so it never runs.
        let mut noise = 0.0;
        if self.nz_live || (p > 0.0 && !self.conga) {
            // Noise VCA ahead of the (linear) filter: equivalent to the VCA
            // after it while the gain moves slowly next to the filter's
            // sub-ms time constants, and it lets the filter go quiet.
            let g = vca_gain(self.nz.v(self.nenv));
            let n = self.noise.tick();
            self.nz.rail = bus.rail;
            self.nz.set_voltage(self.ntrig, p);
            self.nz.set_voltage(self.nsrc, n * g * NOISE_DRIVE);
            self.nz.step();
            noise = self.nz.v(self.nout);
            let filt = self
                .filt
                .iter()
                .fold(0.0f64, |m, &k| m.max(self.nz.v(k).abs()));
            self.nz_live = p > 0.0 || self.nz.v(self.nenv) > 0.05 || filt > 1e-6;
        }
        let level = self.watch.iter().fold(
            if self.nz_live { self.nz.energy() } else { 0.0 },
            |m, &n| m.max(self.c.v(n).abs()),
        );
        self.sleep.observe_level(level);
        self.c.v(self.out) + noise * NOISE_MIX
    }
    fn active(&self) -> bool {
        self.sleep.awake
    }
    fn accent_draw(&self) -> f64 {
        self.pulse.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.sleep.awake, &[&self.c, &self.nz])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 96_000.0;

    fn hit(spec: &TomSpec, conga: bool, accent: f64) -> TomConga {
        let mut t = TomConga::new(SR, spec);
        t.set_switch(conga as u32);
        t.trigger(accent, 0);
        t
    }

    /// Frequency from upward zero crossings of the body in [t0, t1) seconds
    /// (advancing the voice to t1).
    fn freq(t: &mut TomConga, t0: f64, t1: f64) -> f64 {
        let bus = Bus::default();
        let (mut first, mut last, mut n, mut prev) = (0.0, 0.0, 0, 0.0);
        for i in 0..(t1 * SR) as usize {
            t.tick(&bus);
            let v = t.c.v(t.out);
            if i as f64 >= t0 * SR && prev <= 0.0 && v > 0.0 {
                let at = i as f64 - v / (v - prev);
                if n == 0 {
                    first = at;
                }
                last = at;
                n += 1;
            }
            prev = v;
        }
        (n - 1) as f64 * SR / (last - first)
    }

    #[test]
    fn opamp_stays_below_its_rails() {
        for spec in [&LOW, &MID, &HIGH] {
            for conga in [false, true] {
                let mut t = hit(spec, conga, 0.5);
                let mut peak = 0.0f64;
                for _ in 0..(SR * 0.3) as usize {
                    t.tick(&Bus::default());
                    peak = peak.max(t.c.v(t.watch[0]).abs());
                }
                let salt = spec.salt;
                assert!(peak > 2.0 && peak < 8.5, "{salt} conga {conga}: {peak}");
            }
        }
    }

    #[test]
    fn conga_never_runs_the_noise_filter() {
        let mut t = hit(&MID, true, 1.0);
        for _ in 0..(SR * 0.3) as usize {
            t.tick(&Bus::default());
            assert!(!t.nz_live && t.nz.energy() == 0.0);
        }
        // The tom position does use it.
        let mut t = hit(&MID, false, 1.0);
        let mut used = false;
        for _ in 0..(SR * 0.05) as usize {
            t.tick(&Bus::default());
            used |= t.nz.energy() > 0.01;
        }
        assert!(used);
    }

    #[test]
    fn switch_waits_for_the_next_trigger() {
        let mut t = hit(&LOW, false, 0.5);
        let ring = freq(&mut t, 0.1, 0.2);
        t.set_switch(1);
        assert!(!t.is_conga());
        // Still the tom's pitch until struck again.
        let still = freq(&mut t, 0.0, 0.1);
        assert!((still / ring - 1.0).abs() < 0.03, "{ring} {still}");
        t.trigger(0.5, 0);
        assert!(t.is_conga());
        let conga = freq(&mut t, 0.1, 0.2);
        assert!(conga > ring * 1.7, "{ring} {conga}");
    }

    #[test]
    fn pitch_bends_down_after_the_strike() {
        for spec in [&LOW, &MID, &HIGH] {
            for conga in [false, true] {
                let early = freq(&mut hit(spec, conga, 0.5), 0.003, 0.03);
                let late = freq(&mut hit(spec, conga, 0.5), 0.1, 0.2);
                let salt = spec.salt;
                assert!(
                    early > late * 1.04 && early < late * 1.5,
                    "{salt} conga {conga}: {early} then {late}"
                );
            }
        }
    }

    #[test]
    fn sparse_plan_matches_dense_solver() {
        let mut a = TomConga::new(SR, &HIGH);
        let mut b = TomConga::new(SR, &HIGH);
        b.c.dense_only = true;
        b.nz.dense_only = true;
        let mut max_diff = 0.0f64;
        for i in 0..(SR as usize / 2) {
            if i % 12_000 == 0 {
                a.trigger(0.9, 0);
                b.trigger(0.9, 0);
            }
            let bus = Bus::default();
            max_diff = max_diff.max((a.tick(&bus) - b.tick(&bus)).abs());
        }
        assert!(max_diff < 1e-6, "{max_diff}");
    }
}
