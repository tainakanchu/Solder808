//! Cowbell: two Schmitt-trigger square oscillators (~540 Hz and ~800 Hz,
//! frequency from their R and C parts) → band-pass → VCA with a fast and a
//! slow envelope → output coupling. Oscillators and the VCA law are
//! behavioral; the filter and envelopes are solved as a circuit.

use crate::blocks::{mfb_bandpass, peak_env, vca_gain, PartMap, Square};
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

const VOICE_ID: u32 = 4;
const OSC_HZ: [f64; 2] = [540.0, 800.0];
const OSC_C: f64 = 22e-9;
const OSC_K: f64 = 1.4;
const VR_DECAY_MAX: f64 = 470e3;

pub struct Cowbell {
    c: Circuit,
    pm: PartMap,
    sleep: Sleeper,
    osc: [Square; 2],
    sr: f64,
    src: ElemId,
    pulse: Pulse,
    trig_fast: ElemId,
    trig_slow: ElemId,
    env_fast: Node,
    env_slow: Node,
    bp: Node,
    vca: ElemId,
    out: Node,
    r_slow: ElemId,
    vr_decay: usize,
    knob_decay: f64,
}

impl Cowbell {
    pub fn new(sr: f64) -> Self {
        let mut pm = PartMap::default();
        for (i, f) in OSC_HZ.iter().enumerate() {
            let names = [["R1 osc1", "C1 osc1"], ["R2 osc2", "C2 osc2"]][i];
            pm.add(names[0], K::Carbon, 1.0 / (OSC_K * f * OSC_C));
            pm.add(names[1], K::FilmCap, OSC_C);
        }
        let mut cb = CircuitBuilder::new();
        let n_src = cb.node();
        let src = cb.vsource(n_src, GND);
        // ~2.6 kHz band-pass, Q ≈ 1.5.
        let bp = mfb_bandpass(
            &mut cb,
            &mut pm,
            n_src,
            ["R3 BPF in", "C3 BPF", "C4 BPF", "R4 BPF fb"],
            4.1e3,
            10e-9,
            37e3,
        );
        let (trig_fast, env_fast) = peak_env(
            &mut cb,
            &mut pm,
            ["R5 fast charge", "D1 fast", "C5 fast", "R6 fast decay"],
            0.47e-6,
            33e3,
        );
        let n_t = cb.node();
        let n_k = cb.node();
        let env_slow = cb.node();
        let trig_slow = cb.vsource(n_t, GND);
        pm.r(&mut cb, n_t, n_k, "R7 slow charge", 1e3);
        pm.d(&mut cb, n_k, env_slow, "D2 slow");
        pm.c(&mut cb, env_slow, GND, "C6 slow", K::Electrolytic, 1e-6);
        let r_slow = cb.resistor(env_slow, GND, 1.0);
        pm.add("R8 slow (fixed)", K::Carbon, 68e3);
        let vr_decay = pm.add("VR1 decay", K::Pot, VR_DECAY_MAX);
        // VCA output → coupling → load.
        let n_vca = cb.node();
        let out = cb.node();
        let vca = cb.vsource(n_vca, GND);
        pm.c(&mut cb, n_vca, out, "C7 output", K::Electrolytic, 1e-6);
        pm.r(&mut cb, out, GND, "R9 load", 10e3);

        let mut cbell = Self {
            c: cb.build(sr),
            pm,
            sleep: Sleeper::new(sr),
            osc: [
                Square::default(),
                Square {
                    phase: 0.37,
                    inc: 0.0,
                },
            ],
            sr,
            src,
            pulse: Pulse::new(sr),
            trig_fast,
            trig_slow,
            env_fast,
            env_slow,
            bp,
            vca,
            out,
            r_slow,
            vr_decay,
            knob_decay: 0.4,
        };
        cbell.refresh(&Unit::default());
        cbell
    }
}

impl Voice for Cowbell {
    fn parts(&self) -> &[Part] {
        &self.pm.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }
    fn set_knob(&mut self, id: u32, v: f64) {
        if id == 1 {
            self.knob_decay = v;
        }
    }
    fn refresh(&mut self, unit: &Unit) {
        self.pm.parts[self.vr_decay].nominal = VR_DECAY_MAX * self.knob_decay;
        self.pm.refresh(&mut self.c, unit, VOICE_ID);
        for i in 0..2 {
            let r = self.pm.parts[2 * i].effective;
            let c = self.pm.parts[2 * i + 1].effective;
            self.osc[i].inc = 1.0 / (OSC_K * r * c * self.sr);
        }
        let r = self.pm.parts[self.vr_decay - 1].effective + self.pm.parts[self.vr_decay].effective;
        self.c.set_resistance(self.r_slow, r);
    }
    fn trigger(&mut self, accent: f64, _variant: u32) {
        self.pulse.fire(accent);
        self.sleep.wake();
    }
    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake {
            return 0.0;
        }
        let tone = self.osc[0].tick() + self.osc[1].tick();
        let p = self.pulse.next(bus);
        self.c.rail = bus.rail;
        self.c.set_voltage(self.src, tone * 0.5);
        self.c.set_voltage(self.trig_fast, p);
        self.c.set_voltage(self.trig_slow, p * 0.6);
        let g = vca_gain(self.c.v(self.env_fast)) * 5.0 + vca_gain(self.c.v(self.env_slow)) * 9.0;
        self.c.set_voltage(self.vca, self.c.v(self.bp) * g);
        self.c.step();
        let env = self.c.v(self.env_fast).max(self.c.v(self.env_slow));
        if env > 0.21 {
            self.sleep.wake();
        } else {
            self.sleep.observe_level(self.c.v(self.out).abs());
        }
        self.c.v(self.out)
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
