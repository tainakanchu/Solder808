//! Reusable circuit blocks and behavioral sources shared by the newer voices.

use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{apply_unit, Part, PartKind, Unit};

/// Part list plus the circuit elements each part drives.
///
/// A voice split into several circuits keeps one part table: set `target`
/// to the circuit's index before adding its elements, then refresh with
/// `refresh_all` / `push_all` (index order). `refresh` / `push` cover
/// target 0 only.
#[derive(Default)]
pub struct PartMap {
    pub parts: Vec<Part>,
    /// Circuit index recorded with every element added from now on.
    pub target: usize,
    res: Vec<(usize, ElemId, usize)>,
    caps: Vec<(usize, ElemId, usize)>,
    diodes: Vec<(usize, ElemId, usize)>,
}

impl PartMap {
    pub fn add(&mut self, name: &'static str, kind: PartKind, value: f64) -> usize {
        self.parts.push(Part::new(name, kind, value));
        self.parts.len() - 1
    }

    pub fn r(
        &mut self,
        cb: &mut CircuitBuilder,
        a: Node,
        b: Node,
        name: &'static str,
        ohms: f64,
    ) -> usize {
        let e = cb.resistor(a, b, ohms);
        let i = self.add(name, PartKind::Carbon, ohms);
        self.res.push((self.target, e, i));
        i
    }

    pub fn c(
        &mut self,
        cb: &mut CircuitBuilder,
        a: Node,
        b: Node,
        name: &'static str,
        kind: PartKind,
        f: f64,
    ) -> usize {
        let e = cb.capacitor(a, b, f);
        let i = self.add(name, kind, f);
        self.caps.push((self.target, e, i));
        i
    }

    pub fn d(&mut self, cb: &mut CircuitBuilder, a: Node, k: Node, name: &'static str) -> usize {
        let e = cb.diode(a, k, 2.52e-9, 1.752, 0.02585);
        let i = self.add(name, PartKind::Diode, 2.52e-9);
        self.diodes.push((self.target, e, i));
        i
    }

    pub fn refresh(&mut self, c: &mut Circuit, unit: &Unit, voice_id: u32) {
        apply_unit(&mut self.parts, unit, voice_id);
        self.push(c, unit);
    }

    pub fn refresh_all(&mut self, cs: &mut [&mut Circuit], unit: &Unit, voice_id: u32) {
        apply_unit(&mut self.parts, unit, voice_id);
        self.push_all(cs, unit);
    }

    /// Push current effective values without recomputing them.
    pub fn push(&self, c: &mut Circuit, unit: &Unit) {
        self.push_to(0, c, unit);
    }

    pub fn push_all(&self, cs: &mut [&mut Circuit], unit: &Unit) {
        for (k, c) in cs.iter_mut().enumerate() {
            self.push_to(k, c, unit);
        }
    }

    fn push_to(&self, k: usize, c: &mut Circuit, unit: &Unit) {
        for &(_, e, i) in self.res.iter().filter(|r| r.0 == k) {
            c.set_resistance(e, self.parts[i].effective);
        }
        for &(_, e, i) in self.caps.iter().filter(|r| r.0 == k) {
            c.set_capacitance(e, self.parts[i].effective);
        }
        let vt = unit.vt();
        for &(_, e, i) in self.diodes.iter().filter(|r| r.0 == k) {
            let p = &self.parts[i];
            c.set_diode(e, p.effective, p.n, vt);
        }
    }

    /// Effective value of part `i`.
    pub fn eff(&self, i: usize) -> f64 {
        self.parts[i].effective
    }
}

/// Multiple-feedback band-pass around an op-amp. Returns the output node.
pub fn mfb_bandpass(
    cb: &mut CircuitBuilder,
    pm: &mut PartMap,
    input: Node,
    names: [&'static str; 4],
    r_in: f64,
    c: f64,
    r_fb: f64,
) -> Node {
    let a = cb.node();
    let inv = cb.node();
    let out = cb.node();
    pm.r(cb, input, a, names[0], r_in);
    pm.c(cb, a, inv, names[1], PartKind::Ceramic, c);
    pm.c(cb, a, out, names[2], PartKind::Ceramic, c);
    pm.r(cb, out, inv, names[3], r_fb);
    cb.opamp(GND, inv, out);
    out
}

/// Trigger-driven peak detector: vsource → R → diode → C ∥ R.
/// Returns (trigger source, envelope node).
pub fn peak_env(
    cb: &mut CircuitBuilder,
    pm: &mut PartMap,
    names: [&'static str; 4],
    c: f64,
    r_discharge: f64,
) -> (ElemId, Node) {
    let t = cb.node();
    let src = cb.vsource(t, GND);
    (src, peak_det(cb, pm, t, names, 1e3, c, r_discharge))
}

/// Peak detector hanging off an existing trigger node: R → diode → C ∥ R.
/// Returns the envelope node.
pub fn peak_det(
    cb: &mut CircuitBuilder,
    pm: &mut PartMap,
    trig: Node,
    names: [&'static str; 4],
    r_charge: f64,
    c: f64,
    r_discharge: f64,
) -> Node {
    let k = cb.node();
    let env = cb.node();
    pm.r(cb, trig, k, names[0], r_charge);
    pm.d(cb, k, env, names[1]);
    pm.c(cb, env, GND, names[2], PartKind::Electrolytic, c);
    pm.r(cb, env, GND, names[3], r_discharge);
    env
}

/// Bridged-T resonator in an inverting op-amp's feedback path, struck through
/// a series C–R from `drive` (the 808's SD / RS / CL structure):
/// C1 out→mid, C2 mid→inv, Rb mid→gnd, Rf out→inv.
/// f0 ≈ 1 / (2π C √(Rb·Rf)), Q ≈ ½ √(Rf / Rb). Names: C1, C2, Rb, Rf,
/// drive C, drive R. `pos` is the op-amp's + input (GND, or a regeneration
/// divider's tap the caller wires up). Returns the op-amp output node.
#[allow(clippy::too_many_arguments)]
pub fn bridged_t(
    cb: &mut CircuitBuilder,
    pm: &mut PartMap,
    drive: Node,
    pos: Node,
    names: [&'static str; 6],
    c: f64,
    rb: f64,
    rf: f64,
    c_drive: f64,
    r_drive: f64,
) -> Node {
    let ka = cb.node();
    let inv = cb.node();
    let mid = cb.node();
    let out = cb.node();
    pm.c(cb, out, mid, names[0], PartKind::FilmCap, c);
    pm.c(cb, mid, inv, names[1], PartKind::FilmCap, c);
    pm.r(cb, mid, GND, names[2], rb);
    pm.r(cb, out, inv, names[3], rf);
    pm.c(cb, drive, ka, names[4], PartKind::Ceramic, c_drive);
    pm.r(cb, ka, inv, names[5], r_drive);
    cb.opamp(pos, inv, out);
    out
}

/// Squared envelope-to-gain law used by the behavioral VCAs.
pub fn vca_gain(v: f64) -> f64 {
    let x = (v - 0.2).max(0.0) / 8.0;
    x * x
}

/// PolyBLEP square oscillator.
#[derive(Clone, Copy, Default)]
pub struct Square {
    pub phase: f64,
    pub inc: f64,
}

impl Square {
    pub fn tick(&mut self) -> f64 {
        let (t, dt) = (self.phase, self.inc);
        let mut v = if t < 0.5 { 1.0 } else { -1.0 };
        v += blep(t, dt);
        v -= blep((t + 0.5) % 1.0, dt);
        self.phase = (t + dt) % 1.0;
        v
    }
}

fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// xorshift32 white noise in [-1, 1).
pub struct Noise(pub u32);

impl Noise {
    pub fn tick(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f64 / u32::MAX as f64) * 2.0 - 1.0
    }
}
