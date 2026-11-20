//! Reusable circuit blocks and behavioral sources shared by the newer voices.

use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::parts::{apply_unit, Part, PartKind, Unit};

/// Part list plus the circuit elements each part drives.
#[derive(Default)]
pub struct PartMap {
    pub parts: Vec<Part>,
    res: Vec<(ElemId, usize)>,
    caps: Vec<(ElemId, usize)>,
    diodes: Vec<(ElemId, usize)>,
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
        self.res.push((e, i));
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
        self.caps.push((e, i));
        i
    }

    pub fn d(&mut self, cb: &mut CircuitBuilder, a: Node, k: Node, name: &'static str) -> usize {
        let e = cb.diode(a, k, 2.52e-9, 1.752, 0.02585);
        let i = self.add(name, PartKind::Diode, 2.52e-9);
        self.diodes.push((e, i));
        i
    }

    pub fn refresh(&mut self, c: &mut Circuit, unit: &Unit, voice_id: u32) {
        apply_unit(&mut self.parts, unit, voice_id);
        self.push(c, unit);
    }

    /// Push current effective values without recomputing them.
    pub fn push(&self, c: &mut Circuit, unit: &Unit) {
        for &(e, i) in &self.res {
            c.set_resistance(e, self.parts[i].effective);
        }
        for &(e, i) in &self.caps {
            c.set_capacitance(e, self.parts[i].effective);
        }
        let vt = unit.vt();
        for &(e, i) in &self.diodes {
            let p = &self.parts[i];
            c.set_diode(e, p.effective, p.n, vt);
        }
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
    let k = cb.node();
    let env = cb.node();
    let src = cb.vsource(t, GND);
    pm.r(cb, t, k, names[0], 1e3);
    pm.d(cb, k, env, names[1]);
    pm.c(cb, env, GND, names[2], PartKind::Electrolytic, c);
    pm.r(cb, env, GND, names[3], r_discharge);
    (src, env)
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
