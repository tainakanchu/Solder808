//! Cymbal: the hi-hat's six square oscillators (the shared [`MetalBank`],
//! read from [`Bus::metal`]) through two multiple-feedback band-pass filters
//! (~3.4 kHz and ~7.1 kHz), feeding three VCA paths with their own envelopes:
//!
//! - low: the 3.4 kHz band, long envelope set by the Decay knob;
//! - mid: the 7.1 kHz band, medium envelope;
//! - high: the 7.1 kHz band again, short envelope.
//!
//! Each VCA output is high-passed (low and mid ~3.4 kHz, high ~7.2 kHz), which
//! keeps the oscillators' strong low partials, leaking through the band-pass
//! skirts, out of the mix and leaves the high path only the top of the
//! spectrum.
//!
//! That is three band paths with three envelopes from two filters, the split
//! published analyses of the 808 cymbal describe; a third band-pass would
//! only duplicate what the high path's band-pass plus high-pass already does.
//! The Tone pot is a crossfade: its track sits between the low path and the
//! high path, each leg feeding the passive mixing node, so turning it moves
//! resistance from one leg to the other. The mid path mixes at a fixed level.
//!
//! Filters, envelopes and the output network are solved as circuits (the
//! output network is linear, so its factorization is reused between samples);
//! the oscillators and the VCA gain law are behavioral.
//!
//! [`MetalBank`]: crate::hh::MetalBank

use crate::blocks::{mfb_bandpass, peak_det, vca_gain, PartMap};
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{Part, PartKind as K, Unit};
use crate::voice::{audio_taper, Bus, Pulse, Sleeper, Voice};

/// Tolerance seed salt (5 belongs to the common circuitry).
const SALT: u32 = 15;
const VR_DECAY_MAX: f64 = 2.2e6;
const VR_TONE: f64 = 100e3;
/// VCA output scaling (V per V of filter output at unity gain law).
const VCA_K: f64 = 4.0;

pub struct Cymbal {
    bp_lo: Circuit,
    bp_hi: Circuit,
    env: Circuit,
    out: Circuit,
    pm: PartMap,
    pulse: Pulse,
    sleep: Sleeper,
    m_lo: ElemId,
    m_hi: ElemId,
    trig: ElemId,
    v_lo: ElemId,
    v_mid: ElemId,
    v_hi: ElemId,
    e_decay: ElemId,
    e_leg_lo: ElemId,
    e_leg_hi: ElemId,
    p_decay: usize,
    p_vr_decay: usize,
    p_leg_lo: usize,
    p_leg_hi: usize,
    p_tone: usize,
    o_lo: Node,
    o_hi: Node,
    env_lo: Node,
    env_mid: Node,
    env_hi: Node,
    o: Node,
    knob_tone: f64,
    knob_decay: f64,
}

impl Cymbal {
    pub fn new(sr: f64) -> Self {
        let mut pm = PartMap::default();
        // Band-pass filters, one circuit each so one saturating op-amp does
        // not drag the other through extra Newton iterations.
        let mut a = CircuitBuilder::new();
        let n = a.node();
        let m_lo = a.vsource(n, GND);
        let o_lo = mfb_bandpass(
            &mut a,
            &mut pm,
            n,
            ["R1 low BPF in", "C1 low BPF", "C2 low BPF", "R2 low BPF fb"],
            3.3e3,
            4.7e-9,
            30e3,
        );
        pm.target = 1;
        let mut b = CircuitBuilder::new();
        let n = b.node();
        let m_hi = b.vsource(n, GND);
        let o_hi = mfb_bandpass(
            &mut b,
            &mut pm,
            n,
            [
                "R3 high BPF in",
                "C3 high BPF",
                "C4 high BPF",
                "R4 high BPF fb",
            ],
            2.7e3,
            2.2e-9,
            39e3,
        );

        // Envelopes off one trigger line.
        pm.target = 2;
        let mut e = CircuitBuilder::new();
        let t = e.node();
        let trig = e.vsource(t, GND);
        let k = e.node();
        let env_lo = e.node();
        pm.r(&mut e, t, k, "R5 low env charge", 470.0);
        pm.d(&mut e, k, env_lo, "D1 low env");
        pm.c(&mut e, env_lo, GND, "C5 low env", K::Electrolytic, 1e-6);
        let e_decay = e.resistor(env_lo, GND, 1.0);
        let p_decay = pm.add("R6 low decay (fixed)", K::Carbon, 220e3);
        let p_vr_decay = pm.add("VR1 decay", K::Pot, VR_DECAY_MAX);
        let env_mid = peak_det(
            &mut e,
            &mut pm,
            t,
            [
                "R7 mid env charge",
                "D2 mid env",
                "C6 mid env",
                "R8 mid decay",
            ],
            1e3,
            0.47e-6,
            330e3,
        );
        let env_hi = peak_det(
            &mut e,
            &mut pm,
            t,
            [
                "R9 high env charge",
                "D3 high env",
                "C7 high env",
                "R10 high decay",
            ],
            1e3,
            0.22e-6,
            220e3,
        );

        // Output network: high path high-pass, tone crossfade, mixing node,
        // coupling capacitor.
        pm.target = 3;
        let mut c = CircuitBuilder::new();
        let n_lo = c.node();
        let n_mid = c.node();
        let n_hi = c.node();
        let n_l = c.node();
        let n_m = c.node();
        let n_h = c.node();
        let s = c.node();
        let o = c.node();
        let v_lo = c.vsource(n_lo, GND);
        let v_mid = c.vsource(n_mid, GND);
        let v_hi = c.vsource(n_hi, GND);
        pm.c(&mut c, n_lo, n_l, "C8 low HPF", K::FilmCap, 10e-9);
        pm.r(&mut c, n_l, GND, "R11 low HPF", 4.7e3);
        pm.c(&mut c, n_mid, n_m, "C9 mid HPF", K::FilmCap, 10e-9);
        pm.r(&mut c, n_m, GND, "R12 mid HPF", 4.7e3);
        pm.c(&mut c, n_hi, n_h, "C10 high HPF", K::FilmCap, 4.7e-9);
        pm.r(&mut c, n_h, GND, "R13 high HPF", 4.7e3);
        let e_leg_lo = c.resistor(n_l, s, 1.0);
        let e_leg_hi = c.resistor(n_h, s, 1.0);
        let p_leg_lo = pm.add("R14 tone low leg", K::Carbon, 22e3);
        let p_leg_hi = pm.add("R15 tone high leg", K::Carbon, 22e3);
        let p_tone = pm.add("VR2 tone (track)", K::Pot, VR_TONE);
        pm.r(&mut c, n_m, s, "R16 mid mix", 100e3);
        pm.c(&mut c, s, o, "C11 output", K::Electrolytic, 1e-6);
        pm.r(&mut c, o, GND, "R17 load", 10e3);

        let (env, out) = (e.build(sr), c.build(sr));
        let sleep = Sleeper::new(sr, &[&env, &out]);
        let mut cy = Self {
            bp_lo: a.build(sr),
            bp_hi: b.build(sr),
            env,
            out,
            pm,
            pulse: Pulse::new(sr),
            sleep,
            m_lo,
            m_hi,
            trig,
            v_lo,
            v_mid,
            v_hi,
            e_decay,
            e_leg_lo,
            e_leg_hi,
            p_decay,
            p_vr_decay,
            p_leg_lo,
            p_leg_hi,
            p_tone,
            o_lo,
            o_hi,
            env_lo,
            env_mid,
            env_hi,
            o,
            knob_tone: 0.5,
            knob_decay: 0.5,
        };
        cy.refresh(&Unit::default());
        cy
    }
}

impl Voice for Cymbal {
    fn parts(&self) -> &[Part] {
        &self.pm.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }
    fn set_knob(&mut self, id: u32, v: f64) {
        match id {
            1 => self.knob_tone = v,
            2 => self.knob_decay = v,
            _ => {}
        }
    }
    fn refresh(&mut self, unit: &Unit) {
        self.pm.parts[self.p_vr_decay].nominal = VR_DECAY_MAX * audio_taper(self.knob_decay);
        self.sleep.restore(&mut [&mut self.env, &mut self.out]);
        self.pm.refresh_all(
            &mut [
                &mut self.bp_lo,
                &mut self.bp_hi,
                &mut self.env,
                &mut self.out,
            ],
            unit,
            SALT,
        );
        let pm = &self.pm;
        self.env
            .set_resistance(self.e_decay, pm.eff(self.p_decay) + pm.eff(self.p_vr_decay));
        // Wiper toward the high end (tone up) takes track out of the high leg.
        let track = pm.eff(self.p_tone);
        let t = self.knob_tone;
        self.out
            .set_resistance(self.e_leg_lo, pm.eff(self.p_leg_lo) + track * t);
        self.out
            .set_resistance(self.e_leg_hi, pm.eff(self.p_leg_hi) + track * (1.0 - t));
        self.sleep.resume(&[&self.env, &self.out]);
    }
    fn trigger(&mut self, accent: f64, _variant: u32) {
        #[cfg(test)]
        if crate::voice::testing::restart_sources() {
            // The band-passes follow the free-running oscillators.
            self.bp_lo.rest();
            self.bp_hi.rest();
        }
        self.pulse.fire(accent);
        self.sleep.wake(&mut [&mut self.env, &mut self.out]);
    }
    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake() {
            return 0.0;
        }
        let m = bus.metal * 0.5;
        for c in [
            &mut self.bp_lo,
            &mut self.bp_hi,
            &mut self.env,
            &mut self.out,
        ] {
            c.rail = bus.rail;
        }
        self.bp_lo.set_voltage(self.m_lo, m);
        self.bp_lo.step();
        self.bp_hi.set_voltage(self.m_hi, m);
        self.bp_hi.step();
        let p = self.pulse.next(bus);
        self.env.set_voltage(self.trig, p);
        self.env.step();
        let (el, em, eh) = (
            self.env.v(self.env_lo),
            self.env.v(self.env_mid),
            self.env.v(self.env_hi),
        );
        let lo = self.bp_lo.v(self.o_lo);
        let hi = self.bp_hi.v(self.o_hi);
        self.out.set_voltage(self.v_lo, lo * vca_gain(el) * VCA_K);
        self.out.set_voltage(self.v_mid, hi * vca_gain(em) * VCA_K);
        self.out.set_voltage(self.v_hi, hi * vca_gain(eh) * VCA_K);
        self.out.step();
        self.sleep.hear(self.out.v(self.o))
    }
    fn active(&self) -> bool {
        self.sleep.awake()
    }
    fn housekeep(&mut self) {
        // Past the pulse the envelopes only fall, so the output only fades.
        // The band-passes are left as they are while asleep.
        self.sleep
            .housekeep(&[&self.env, &self.out], !self.pulse.busy());
    }
    fn set_quiet(&mut self, volts: f64) {
        self.sleep.set_threshold(volts);
    }
    fn accent_draw(&self) -> f64 {
        self.pulse.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(
            self.sleep.awake(),
            // The band-passes follow the free-running oscillators: a
            // steady load, part of the nominal rail.
            &[&self.env, &self.out],
        )
    }
}
