//! Hi-hat: six Schmitt-trigger square oscillators (behavioral, frequency set by
//! their R and C parts) summed into a multiple-feedback band-pass op-amp
//! filter, a VCA fed by closed / open envelope peak detectors, and a final RC
//! high-pass. A closed trigger chokes the open envelope like the original.
//!
//! The six oscillators are the 808's shared "metal" source: the cymbal filters
//! the same bank. They live in [`MetalBank`], owned by the engine, which
//! publishes their sum on [`Bus::metal`]; their R and C parts stay in this
//! voice's part table (indices 0..12), so tolerance, aging and manual part
//! edits retune the bank for both voices at once.

use crate::blocks::Square;
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{apply_unit, Part, PartKind as K, Unit};
use crate::voice::{audio_taper, Bus, Pulse, Sleeper, Voice};

const VOICE_ID: u32 = 2;
/// Nominal oscillator frequencies of the 808 metallic source.
const OSC_HZ: [f64; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
const OSC_C: f64 = 22e-9;
/// Period of the Schmitt oscillator ≈ K * R * C.
const OSC_K: f64 = 1.4;
const VR_DECAY_MAX: f64 = 600e3;

/// The six square oscillators shared by the hi-hat and the cymbal.
///
/// Why one bank owned by the engine rather than a copy per voice: on the 808
/// both voices tap the same oscillators, so a hi-hat and a cymbal struck
/// together beat against identical frequencies and phases. A second bank
/// computed from the same formula would need its own parts (different
/// tolerance draws, so slightly different pitches) and its own phases, and
/// the pair would no longer sound like one piece of metal. The bank only runs
/// while either voice is awake; its phases simply hold while both sleep, which
/// is inaudible because a free-running bank has arbitrary phase anyway.
pub struct MetalBank {
    osc: [Square; 6],
}

impl Default for MetalBank {
    fn default() -> Self {
        let ph = [0.0, 0.31, 0.57, 0.11, 0.83, 0.42];
        Self {
            osc: ph.map(|phase| Square { phase, inc: 0.0 }),
        }
    }
}

impl MetalBank {
    /// Retune from the hi-hat's part table (R1, C1, … R6, C6 at indices 0..12).
    pub fn tune(&mut self, hh_parts: &[Part], sr: f64) {
        for (i, o) in self.osc.iter_mut().enumerate() {
            let r = hh_parts[2 * i].effective;
            let c = hh_parts[2 * i + 1].effective;
            o.inc = 1.0 / (OSC_K * r * c * sr);
        }
    }

    /// Sum of the six ±1 squares.
    pub fn tick(&mut self) -> f64 {
        self.osc.iter_mut().map(|o| o.tick()).sum()
    }

    /// Current frequencies (Hz) at sample rate `sr`.
    pub fn hz(&self, sr: f64) -> [f64; 6] {
        self.osc.map(|o| o.inc * sr)
    }
}

pub struct HiHat {
    a: Circuit,
    b: Circuit,
    parts: Vec<Part>,
    pulse_ch: Pulse,
    pulse_oh: Pulse,
    sleep: Sleeper,
    sr: f64,
    metal: ElemId,
    trig_ch: ElemId,
    trig_oh: ElemId,
    choke: ElemId,
    choke_left: u32,
    vca_in: ElemId,
    map_a: Vec<(ElemId, usize, bool)>,
    map_b: Vec<(ElemId, usize, bool)>,
    diodes: Vec<(ElemId, usize)>,
    r_decay: ElemId,
    bp: Node,
    env_ch: Node,
    env_oh: Node,
    out: Node,
    knob_decay: f64,
}

impl HiHat {
    pub fn new(sr: f64) -> Self {
        let mut parts: Vec<Part> = Vec::new();
        let part = |parts: &mut Vec<Part>, name, kind, v| {
            parts.push(Part::new(name, kind, v));
            parts.len() - 1
        };
        const R_NAMES: [&str; 6] = [
            "R1 osc1", "R2 osc2", "R3 osc3", "R4 osc4", "R5 osc5", "R6 osc6",
        ];
        const C_NAMES: [&str; 6] = [
            "C1 osc1", "C2 osc2", "C3 osc3", "C4 osc4", "C5 osc5", "C6 osc6",
        ];
        for (i, f) in OSC_HZ.iter().enumerate() {
            part(&mut parts, R_NAMES[i], K::Carbon, 1.0 / (OSC_K * f * OSC_C));
            part(&mut parts, C_NAMES[i], K::FilmCap, OSC_C);
        }

        // Circuit A: band-pass on the metallic sum + envelopes.
        let mut cb = CircuitBuilder::new();
        let mut map_a = Vec::new();
        let mut diodes = Vec::new();
        let n_metal = cb.node();
        let n_a = cb.node();
        let n_inv = cb.node();
        let n_bp = cb.node();
        let metal = cb.vsource(n_metal, GND);
        let e = cb.resistor(n_metal, n_a, 1.0);
        map_a.push((e, part(&mut parts, "R7 BPF in", K::Carbon, 5.6e3), false));
        let e = cb.capacitor(n_a, n_inv, 1.0);
        map_a.push((e, part(&mut parts, "C7 BPF", K::Ceramic, 1e-9), true));
        let e = cb.capacitor(n_a, n_bp, 1.0);
        map_a.push((e, part(&mut parts, "C8 BPF", K::Ceramic, 1e-9), true));
        let e = cb.resistor(n_bp, n_inv, 1.0);
        map_a.push((e, part(&mut parts, "R8 BPF fb", K::Carbon, 91e3), false));
        cb.opamp(GND, n_inv, n_bp);

        let mut env = |cb: &mut CircuitBuilder,
                       parts: &mut Vec<Part>,
                       map: &mut Vec<_>,
                       names: [&'static str; 3]| {
            let n_t = cb.node();
            let n_c = cb.node();
            let n_e = cb.node();
            let t = cb.vsource(n_t, GND);
            let e = cb.resistor(n_t, n_c, 1.0);
            map.push((e, part(parts, names[0], K::Carbon, 1e3), false));
            let d = cb.diode(n_c, n_e, 2.52e-9, 1.752, 0.02585);
            diodes.push((d, part(parts, names[1], K::Diode, 2.52e-9)));
            let e = cb.capacitor(n_e, GND, 1.0);
            map.push((e, part(parts, names[2], K::Electrolytic, 1e-6), true));
            (t, n_e)
        };
        let (trig_ch, env_ch) = env(
            &mut cb,
            &mut parts,
            &mut map_a,
            ["R9 CH charge", "D1 CH env", "C9 CH env"],
        );
        let e = cb.resistor(env_ch, GND, 1.0);
        map_a.push((e, part(&mut parts, "R10 CH decay", K::Carbon, 33e3), false));
        let (trig_oh, env_oh) = env(
            &mut cb,
            &mut parts,
            &mut map_a,
            ["R11 OH charge", "D2 OH env", "C10 OH env"],
        );
        let r_fixed = part(&mut parts, "R12 OH decay (fixed)", K::Carbon, 82e3);
        let vr = part(&mut parts, "VR1 OH decay", K::Pot, VR_DECAY_MAX);
        debug_assert_eq!(vr, r_fixed + 1);
        let r_decay = cb.resistor(env_oh, GND, 1.0);
        let choke = cb.resistor(env_oh, GND, 1e9);

        // Circuit B: output high-pass after the VCA.
        let mut cb2 = CircuitBuilder::new();
        let mut map_b = Vec::new();
        let n_v = cb2.node();
        let n_out = cb2.node();
        let vca_in = cb2.vsource(n_v, GND);
        let e = cb2.capacitor(n_v, n_out, 1.0);
        map_b.push((e, part(&mut parts, "C11 HPF", K::FilmCap, 10e-9), true));
        let e = cb2.resistor(n_out, GND, 1.0);
        map_b.push((e, part(&mut parts, "R13 HPF", K::Carbon, 2.2e3), false));

        let mut hh = Self {
            a: cb.build(sr),
            b: cb2.build(sr),
            parts,
            pulse_ch: Pulse::new(sr),
            pulse_oh: Pulse::new(sr),
            sleep: Sleeper::new(sr),
            sr,
            metal,
            trig_ch,
            trig_oh,
            choke,
            choke_left: 0,
            vca_in,
            map_a,
            map_b,
            diodes,
            r_decay,
            bp: n_bp,
            env_ch,
            env_oh,
            out: n_out,
            knob_decay: 0.4,
        };
        hh.refresh(&Unit::default());
        hh
    }

    fn r_decay_index(&self) -> usize {
        self.parts
            .iter()
            .position(|p| p.name == "R12 OH decay (fixed)")
            .unwrap_or(0)
    }
}

impl Voice for HiHat {
    fn parts(&self) -> &[Part] {
        &self.parts
    }
    fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.parts
    }

    fn set_knob(&mut self, id: u32, v: f64) {
        if id == 1 {
            self.knob_decay = v;
        }
    }

    fn refresh(&mut self, unit: &Unit) {
        let ri = self.r_decay_index();
        self.parts[ri + 1].nominal = VR_DECAY_MAX * audio_taper(self.knob_decay);
        apply_unit(&mut self.parts, unit, VOICE_ID);
        for &(id, pi, cap) in &self.map_a {
            let v = self.parts[pi].effective;
            if cap {
                self.a.set_capacitance(id, v);
            } else {
                self.a.set_resistance(id, v);
            }
        }
        for &(id, pi, cap) in &self.map_b {
            let v = self.parts[pi].effective;
            if cap {
                self.b.set_capacitance(id, v);
            } else {
                self.b.set_resistance(id, v);
            }
        }
        let rd = self.parts[ri].effective + self.parts[ri + 1].effective;
        self.a.set_resistance(self.r_decay, rd);
        let vt = unit.vt();
        for &(id, pi) in &self.diodes {
            let p = &self.parts[pi];
            self.a.set_diode(id, p.effective, p.n, vt);
        }
    }

    fn trigger(&mut self, accent: f64, variant: u32) {
        if variant == 0 {
            self.pulse_ch.fire(accent);
            self.choke_left = (self.sr * 3e-3) as u32;
            self.a.set_resistance(self.choke, 100.0);
        } else {
            self.pulse_oh.fire(accent);
        }
        self.sleep.wake();
    }

    fn tick(&mut self, bus: &Bus) -> f64 {
        if !self.sleep.awake {
            return 0.0;
        }
        if self.choke_left > 0 {
            self.choke_left -= 1;
            if self.choke_left == 0 {
                self.a.set_resistance(self.choke, 1e9);
            }
        }
        let ch = self.pulse_ch.next(bus);
        let oh = self.pulse_oh.next(bus);
        self.a.rail = bus.rail;
        self.b.rail = bus.rail;
        self.a.set_voltage(self.metal, bus.metal * 0.5);
        self.a.set_voltage(self.trig_ch, ch);
        self.a.set_voltage(self.trig_oh, oh);
        self.a.step();
        // Behavioral VCA: gain grows with the square of the envelope voltage.
        let g = |v: f64| {
            let x = (v - 0.2).max(0.0) / 8.0;
            x * x * 4.0
        };
        let gain = g(self.a.v(self.env_ch)) + g(self.a.v(self.env_oh));
        self.b.set_voltage(self.vca_in, self.a.v(self.bp) * gain);
        self.b.step();
        // Only the envelopes and output decide sleep; the oscillators never stop.
        let env = self.a.v(self.env_ch).abs().max(self.a.v(self.env_oh).abs());
        if env < 0.21 && self.b.energy() < 1e-4 {
            self.sleep.observe(&[&self.b]);
        } else {
            self.sleep.wake();
        }
        self.b.v(self.out)
    }

    fn active(&self) -> bool {
        self.sleep.awake
    }

    fn accent_draw(&self) -> f64 {
        self.pulse_ch.draw() + self.pulse_oh.draw()
    }
    fn supply_current(&self) -> f64 {
        crate::voice::supply_draw(self.sleep.awake, &[&self.a, &self.b])
    }
}
