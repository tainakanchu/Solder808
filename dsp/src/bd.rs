//! Bass drum: trigger → peak detector (pitch envelope) + click → bridged-T
//! resonator around an op-amp → tone low-pass → output coupling capacitor.
//!
//! The bridged-T sets the pitch; decay comes from regenerative feedback into
//! the op-amp's + input, which deepens the resonance without moving the pitch
//! much. Topology follows the 808 BD signal flow; values are representative,
//! not copied from the service manual.

use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{apply_unit, Part, PartKind as K, Unit};
use crate::voice::{Bus, Pulse, Sleeper, Voice};

const VOICE_ID: u32 = 0;

const VR_TUNE_MAX: f64 = 500.0;
const VR_DECAY_MAX: f64 = 75e3;
const VR_TONE_MAX: f64 = 15e3;

#[derive(Clone, Copy)]
enum Target {
    R(ElemId),
    C(ElemId),
    /// Resistor made of a fixed part plus a pot (two part indices).
    RSum(ElemId, usize),
}

pub struct BassDrum {
    c: Circuit,
    parts: Vec<Part>,
    pulse: Pulse,
    sleep: Sleeper,
    trig: ElemId,
    map: Vec<(Target, usize)>,
    diodes: Vec<(ElemId, usize)>,
    vr_tune: usize,
    vr_decay: usize,
    vr_tone: usize,
    out: Node,
    knob_tune: f64,
    knob_decay: f64,
    knob_tone: f64,
}

impl BassDrum {
    pub fn new(sr: f64) -> Self {
        let mut cb = CircuitBuilder::new();
        let mut parts: Vec<Part> = Vec::new();
        let mut map = Vec::new();
        let mut diodes = Vec::new();
        let part = |parts: &mut Vec<Part>, name, kind, v| {
            parts.push(Part::new(name, kind, v));
            parts.len() - 1
        };

        let n_trig = cb.node();
        let n_chg = cb.node();
        let n_env = cb.node();
        let n_d = cb.node();
        let n_mid = cb.node();
        let n_inv = cb.node();
        let n_pos = cb.node();
        let n_out = cb.node();
        let n_ka = cb.node();
        let n_tone = cb.node();
        let n_outc = cb.node();
        let trig = cb.vsource(n_trig, GND);

        // Bridged-T resonator in the op-amp feedback loop.
        let e = cb.capacitor(n_out, n_mid, 1.0);
        map.push((
            Target::C(e),
            part(&mut parts, "C1 resonator", K::FilmCap, 0.1e-6),
        ));
        let e = cb.capacitor(n_mid, n_inv, 1.0);
        map.push((
            Target::C(e),
            part(&mut parts, "C2 resonator", K::FilmCap, 0.1e-6),
        ));
        let e = cb.resistor(n_mid, GND, 1.0);
        let r1 = part(&mut parts, "R1 tune", K::Carbon, 2.2e3);
        let vr_tune = part(&mut parts, "VR1 tune trim", K::Pot, VR_TUNE_MAX);
        map.push((Target::RSum(e, vr_tune), r1));
        let e = cb.resistor(n_out, n_inv, 1.0);
        map.push((
            Target::R(e),
            part(&mut parts, "R2 bridge", K::Carbon, 330e3),
        ));
        cb.opamp(n_pos, n_inv, n_out);

        // Regenerative feedback (decay).
        let e = cb.resistor(n_out, n_pos, 1.0);
        let r3 = part(&mut parts, "R3 regen", K::Carbon, 84e3);
        let vr_decay = part(&mut parts, "VR2 decay", K::Pot, VR_DECAY_MAX);
        map.push((Target::RSum(e, vr_decay), r3));
        let e = cb.resistor(n_pos, GND, 1.0);
        map.push((
            Target::R(e),
            part(&mut parts, "R4 regen ground", K::Carbon, 1e3),
        ));

        // Pitch envelope: peak detector charged by the trigger, biasing D2
        // whose conductance pulls the bridged-T's tune node.
        let e = cb.resistor(n_trig, n_chg, 1.0);
        map.push((
            Target::R(e),
            part(&mut parts, "R5 env charge", K::Carbon, 1e3),
        ));
        let d = cb.diode(n_chg, n_env, 2.52e-9, 1.752, 0.02585);
        diodes.push((d, part(&mut parts, "D1 env detector", K::Diode, 2.52e-9)));
        let e = cb.capacitor(n_env, GND, 1.0);
        map.push((
            Target::C(e),
            part(&mut parts, "C3 pitch env", K::Electrolytic, 2.2e-6),
        ));
        let e = cb.resistor(n_env, GND, 1.0);
        map.push((
            Target::R(e),
            part(&mut parts, "R6 env discharge", K::Carbon, 22e3),
        ));
        let e = cb.resistor(n_env, n_d, 1.0);
        map.push((
            Target::R(e),
            part(&mut parts, "R7 pitch bias", K::Carbon, 220e3),
        ));
        let d = cb.diode(n_d, n_mid, 2.52e-9, 1.752, 0.02585);
        diodes.push((d, part(&mut parts, "D2 pitch", K::Diode, 2.52e-9)));

        // Click: differentiated trigger into the summing node.
        let e = cb.capacitor(n_trig, n_ka, 1.0);
        map.push((
            Target::C(e),
            part(&mut parts, "C4 click", K::Ceramic, 10e-9),
        ));
        let e = cb.resistor(n_ka, n_inv, 1.0);
        map.push((Target::R(e), part(&mut parts, "R8 click", K::Carbon, 22e3)));

        // Tone low-pass and output coupling.
        let e = cb.resistor(n_out, n_tone, 1.0);
        let r9 = part(&mut parts, "R9 tone", K::Carbon, 680.0);
        let vr_tone = part(&mut parts, "VR3 tone", K::Pot, VR_TONE_MAX);
        map.push((Target::RSum(e, vr_tone), r9));
        let e = cb.capacitor(n_tone, GND, 1.0);
        map.push((Target::C(e), part(&mut parts, "C5 tone", K::FilmCap, 47e-9)));
        let e = cb.capacitor(n_tone, n_outc, 1.0);
        map.push((
            Target::C(e),
            part(&mut parts, "C6 output", K::Electrolytic, 1e-6),
        ));
        let e = cb.resistor(n_outc, GND, 1.0);
        map.push((Target::R(e), part(&mut parts, "R10 load", K::Carbon, 10e3)));

        let c = cb.build(sr);
        let sleep = Sleeper::new(sr, &[&c]);
        let mut bd = Self {
            c,
            parts,
            pulse: Pulse::new(sr),
            sleep,
            trig,
            map,
            diodes,
            vr_tune,
            vr_decay,
            vr_tone,
            out: n_outc,
            knob_tune: 0.5,
            knob_decay: 0.6,
            knob_tone: 0.5,
        };
        bd.refresh(&Unit::default());
        bd
    }
}

impl Voice for BassDrum {
    fn parts(&self) -> &[Part] {
        &self.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.parts
    }

    fn set_knob(&mut self, id: u32, v: f64) {
        match id {
            1 => self.knob_tune = v,
            2 => self.knob_tone = v,
            3 => self.knob_decay = v,
            _ => {}
        }
    }

    fn refresh(&mut self, unit: &Unit) {
        self.parts[self.vr_tune].nominal = VR_TUNE_MAX * (1.0 - self.knob_tune);
        self.parts[self.vr_decay].nominal = VR_DECAY_MAX * (1.0 - self.knob_decay);
        self.parts[self.vr_tone].nominal = VR_TONE_MAX * (1.0 - self.knob_tone);
        apply_unit(&mut self.parts, unit, VOICE_ID);
        self.sleep.restore(&mut [&mut self.c]);
        for &(t, pi) in &self.map {
            let v = self.parts[pi].effective;
            match t {
                Target::R(id) => self.c.set_resistance(id, v),
                Target::C(id) => self.c.set_capacitance(id, v),
                Target::RSum(id, pot) => self.c.set_resistance(id, v + self.parts[pot].effective),
            }
        }
        let vt = unit.vt();
        for &(id, pi) in &self.diodes {
            let p = &self.parts[pi];
            self.c.set_diode(id, p.effective, p.n, vt);
        }
        self.sleep.resume(&[&self.c]);
    }

    fn trigger(&mut self, accent: f64, _variant: u32) {
        self.pulse.fire(accent);
        self.sleep.wake(&mut [&mut self.c]);
    }

    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake() {
            return 0.0;
        }
        let v = self.pulse.next(bus);
        self.c.set_voltage(self.trig, v);
        self.c.rail = bus.rail;
        self.c.step();
        self.sleep.hear(self.c.v(self.out))
    }

    fn active(&self) -> bool {
        self.sleep.awake()
    }

    fn housekeep(&mut self) {
        self.sleep.housekeep(&[&self.c], !self.pulse.busy());
    }

    fn set_quiet(&mut self, volts: f64) {
        self.sleep.set_threshold(volts);
    }

    fn accent_draw(&self) -> f64 {
        self.pulse.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.sleep.awake(), &[&self.c])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::Bus;

    #[test]
    fn newton_iterations_at_onset() {
        let sr = 96_000.0;
        let mut bd = BassDrum::new(sr);
        let mut hist = [0usize; 41];
        let mut first = Vec::new();
        for i in 0..(sr as usize / 2) {
            if i == 100 {
                bd.trigger(0.35, 0);
            }
            bd.tick(&Bus::default());
            hist[bd.c.last_iters.min(40)] += 1;
            if (100..400).contains(&i) {
                first.push(bd.c.last_iters);
            }
        }
        println!(
            "{:?}",
            hist.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .collect::<Vec<_>>()
        );
        println!("{:?}", first);
        println!("replans {}", bd.c.replans);
    }
}
