//! Small modified-nodal-analysis (MNA) circuit solver.
//!
//! Capacitors use the trapezoidal rule; diodes and op-amps are solved with
//! Newton-Raphson every sample. All buffers are allocated when the circuit is
//! built, so `step` never allocates (required inside an AudioWorklet).

pub type Node = usize;
pub const GND: Node = 0;

const GMIN: f64 = 1e-12;
const MAX_ITER: usize = 40;
const TOL_V: f64 = 1e-6;
const MAX_PLANS: usize = 4;

#[derive(Clone, Copy, Debug)]
enum Elem {
    R {
        a: Node,
        b: Node,
        g: f64,
    },
    C {
        a: Node,
        b: Node,
        c: f64,
        geq: f64,
        j: f64,
        v_prev: f64,
        i_prev: f64,
    },
    V {
        a: Node,
        b: Node,
        v: f64,
        row: usize,
    },
    /// Independent current source driving `i` amps from `a` into `b`.
    I {
        a: Node,
        b: Node,
        i: f64,
    },
    D {
        a: Node,
        b: Node,
        is: f64,
        nvt: f64,
        vcrit: f64,
        vd: f64,
    },
    /// Op-amp as a saturating VCVS: v_out = vsat * tanh(gain * (v+ - v-) / vsat).
    OpAmp {
        inp: Node,
        inn: Node,
        out: Node,
        gain: f64,
        vsat: f64,
        row: usize,
        vd: f64,
    },
}

/// Handle to an element whose value can change at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElemId(usize);

pub struct CircuitBuilder {
    nodes: usize,
    vsources: usize,
    elems: Vec<Elem>,
}

impl Default for CircuitBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBuilder {
    pub fn new() -> Self {
        Self {
            nodes: 1,
            vsources: 0,
            elems: Vec::new(),
        }
    }

    pub fn node(&mut self) -> Node {
        self.nodes += 1;
        self.nodes - 1
    }

    fn push(&mut self, e: Elem) -> ElemId {
        self.elems.push(e);
        ElemId(self.elems.len() - 1)
    }

    pub fn resistor(&mut self, a: Node, b: Node, ohms: f64) -> ElemId {
        self.push(Elem::R {
            a,
            b,
            g: 1.0 / ohms,
        })
    }

    pub fn capacitor(&mut self, a: Node, b: Node, farads: f64) -> ElemId {
        self.push(Elem::C {
            a,
            b,
            c: farads,
            geq: 0.0,
            j: 0.0,
            v_prev: 0.0,
            i_prev: 0.0,
        })
    }

    pub fn isource(&mut self, a: Node, b: Node) -> ElemId {
        self.push(Elem::I { a, b, i: 0.0 })
    }

    pub fn vsource(&mut self, a: Node, b: Node) -> ElemId {
        let row = self.vsources;
        self.vsources += 1;
        self.push(Elem::V { a, b, v: 0.0, row })
    }

    pub fn diode(&mut self, anode: Node, cathode: Node, is: f64, n: f64, vt: f64) -> ElemId {
        let mut e = Elem::D {
            a: anode,
            b: cathode,
            is,
            nvt: n * vt,
            vcrit: 0.0,
            vd: 0.0,
        };
        update_vcrit(&mut e);
        self.push(e)
    }

    pub fn opamp(&mut self, inp: Node, inn: Node, out: Node) -> ElemId {
        let row = self.vsources;
        self.vsources += 1;
        self.push(Elem::OpAmp {
            inp,
            inn,
            out,
            gain: 2.0e4,
            vsat: 10.5,
            row,
            vd: 0.0,
        })
    }

    pub fn build(self, sample_rate: f64) -> Circuit {
        let nn = self.nodes - 1;
        let size = nn + self.vsources;
        let mut c = Circuit {
            nn,
            size,
            dt: 1.0 / sample_rate,
            elems: self.elems,
            a_lin: vec![0.0; size * size],
            a: vec![0.0; size * size],
            b: vec![0.0; size],
            x: vec![0.0; size],
            perm: vec![0; size],
            pattern: vec![false; size * size],
            plans: Vec::with_capacity(MAX_PLANS),
            rail: 1.0,
            dense_only: false,
            lu: vec![0.0; size * size],
            bp: vec![0.0; size],
            dirty: true,
            last_iters: 0,
            replans: 0,
        };
        c.build_pattern();
        c.set_sample_rate(sample_rate);
        c
    }
}

fn update_vcrit(e: &mut Elem) {
    if let Elem::D { is, nvt, vcrit, .. } = e {
        *vcrit = *nvt * (*nvt / (std::f64::consts::SQRT_2 * *is)).ln();
    }
}

pub struct Circuit {
    nn: usize,
    size: usize,
    dt: f64,
    elems: Vec<Elem>,
    a_lin: Vec<f64>,
    a: Vec<f64>,
    b: Vec<f64>,
    x: Vec<f64>,
    perm: Vec<usize>,
    /// Structural non-zeros of the MNA matrix (fixed by the topology).
    pattern: Vec<bool>,
    /// Elimination plans for the pivot orders seen so far (most recent first).
    plans: Vec<LuPlan>,
    /// Supply rail relative to nominal; scales every op-amp's output swing.
    pub rail: f64,
    /// Testing aid: always use the dense pivoting solver.
    pub(crate) dense_only: bool,
    lu: Vec<f64>,
    bp: Vec<f64>,
    dirty: bool,
    pub last_iters: usize,
    /// Number of times the sparse plan was rebuilt (diagnostics).
    pub replans: usize,
}

impl Circuit {
    pub fn set_sample_rate(&mut self, sr: f64) {
        self.dt = 1.0 / sr;
        for e in &mut self.elems {
            if let Elem::C { c, geq, .. } = e {
                *geq = 2.0 * *c / self.dt;
            }
        }
        self.dirty = true;
    }

    pub fn set_resistance(&mut self, id: ElemId, ohms: f64) {
        if let Elem::R { g, .. } = &mut self.elems[id.0] {
            let ng = 1.0 / ohms.max(1e-3);
            if *g != ng {
                *g = ng;
                self.dirty = true;
            }
        }
    }

    pub fn set_capacitance(&mut self, id: ElemId, farads: f64) {
        let dt = self.dt;
        if let Elem::C { c, geq, .. } = &mut self.elems[id.0] {
            if *c != farads {
                *c = farads.max(1e-15);
                *geq = 2.0 * *c / dt;
                self.dirty = true;
            }
        }
    }

    pub fn set_current(&mut self, id: ElemId, amps: f64) {
        if let Elem::I { i, .. } = &mut self.elems[id.0] {
            *i = amps;
        }
    }

    pub fn set_voltage(&mut self, id: ElemId, volts: f64) {
        if let Elem::V { v, .. } = &mut self.elems[id.0] {
            *v = volts;
        }
    }

    pub fn set_diode(&mut self, id: ElemId, is_new: f64, n: f64, vt: f64) {
        let e = &mut self.elems[id.0];
        if let Elem::D { is, nvt, .. } = e {
            *is = is_new;
            *nvt = n * vt;
        }
        update_vcrit(e);
    }

    pub fn v(&self, n: Node) -> f64 {
        if n == GND {
            0.0
        } else {
            self.x[n - 1]
        }
    }

    /// Largest absolute node voltage; used to decide when a voice is silent.
    pub fn energy(&self) -> f64 {
        self.x[..self.nn].iter().fold(0.0f64, |m, v| m.max(v.abs()))
    }

    fn rebuild_linear(&mut self) {
        let n = self.size;
        let nn = self.nn;
        let a = &mut self.a_lin;
        a.iter_mut().for_each(|v| *v = 0.0);
        for i in 0..nn {
            a[i * n + i] += GMIN;
        }
        for e in &self.elems {
            match *e {
                Elem::R { a: p, b: q, g } => stamp_g(a, n, p, q, g),
                Elem::I { .. } => {}
                Elem::C {
                    a: p, b: q, geq, ..
                } => stamp_g(a, n, p, q, geq),
                Elem::V {
                    a: p, b: q, row, ..
                } => {
                    let r = nn + row;
                    if p != GND {
                        a[(p - 1) * n + r] += 1.0;
                        a[r * n + p - 1] += 1.0;
                    }
                    if q != GND {
                        a[(q - 1) * n + r] -= 1.0;
                        a[r * n + q - 1] -= 1.0;
                    }
                }
                Elem::OpAmp { out, row, .. } => {
                    let r = nn + row;
                    a[(out - 1) * n + r] += 1.0;
                    a[r * n + out - 1] += 1.0;
                }
                Elem::D { .. } => {}
            }
        }
        self.dirty = false;
    }

    fn build_pattern(&mut self) {
        let n = self.size;
        let nn = self.nn;
        let p = &mut self.pattern;
        let mut mark = |i: Node, j: Node| {
            if i != GND && j != GND {
                p[(i - 1) * n + j - 1] = true;
            }
        };
        for i in 1..=nn {
            mark(i, i);
        }
        let mut vs = Vec::new();
        for e in &self.elems {
            match *e {
                Elem::R { a, b, .. } | Elem::C { a, b, .. } | Elem::D { a, b, .. } => {
                    mark(a, a);
                    mark(b, b);
                    mark(a, b);
                    mark(b, a);
                }
                Elem::V { a, b, row, .. } => vs.push((nn + row + 1, [a, b])),
                Elem::I { .. } => {}
                Elem::OpAmp {
                    inp, inn, out, row, ..
                } => {
                    let r = nn + row + 1;
                    vs.push((r, [out, GND]));
                    mark(r, inp);
                    mark(r, inn);
                }
            }
        }
        for (r, nodes) in vs {
            for nd in nodes {
                mark(nd, r);
                mark(r, nd);
            }
        }
    }

    /// Solve `a x = b` (solution into `b`) with a cached sparse elimination
    /// plan; falls back to dense partial pivoting and re-plans when a pivot
    /// of the cached ordering becomes unreliable.
    fn solve_step(&mut self) -> bool {
        let n = self.size;
        // Diodes switching on and off flip between a few pivot orders; try
        // every cached plan before falling back to dense pivoting.
        for k in 0..self.plans.len() {
            if self.plans[k].solve(&self.a, &mut self.b, &mut self.lu, &mut self.bp, n) {
                if k != 0 {
                    self.plans.swap(0, k);
                }
                return true;
            }
        }
        if !solve(&mut self.a, &mut self.b, &mut self.perm, n) {
            return false;
        }
        if self.dense_only {
            return true;
        }
        self.replans += 1;
        if self.plans.len() == MAX_PLANS {
            self.plans.pop();
        }
        self.plans
            .insert(0, LuPlan::new(&self.pattern, &self.perm, n));
        true
    }

    fn nonlinear_settled(&self) -> bool {
        self.elems.iter().all(|e| match *e {
            Elem::D { a, b, nvt, vd, .. } => {
                ((node_v(&self.x, a) - node_v(&self.x, b)) - vd).abs() < 1e-3 * nvt
            }
            Elem::OpAmp {
                inp, inn, gain, vd, ..
            } => (gain * ((node_v(&self.x, inp) - node_v(&self.x, inn)) - vd)).abs() < 1e-4,
            _ => true,
        })
    }

    /// Advance the circuit by one sample.
    pub fn step(&mut self) {
        if self.dirty {
            self.rebuild_linear();
        }
        let n = self.size;
        let nn = self.nn;
        // Capacitor history currents for this step.
        for e in &mut self.elems {
            if let Elem::C {
                geq,
                j,
                v_prev,
                i_prev,
                ..
            } = e
            {
                *j = *geq * *v_prev + *i_prev;
            }
        }
        let rail = self.rail;
        let mut iters = 0;
        loop {
            iters += 1;
            self.a.copy_from_slice(&self.a_lin);
            self.b.iter_mut().for_each(|v| *v = 0.0);
            let mut limited = false;
            for e in &mut self.elems {
                match e {
                    Elem::C { a: p, b: q, j, .. } => stamp_i(&mut self.b, *p, *q, *j),
                    Elem::V { v, row, .. } => self.b[nn + *row] = *v,
                    Elem::I { a: p, b: q, i } => stamp_i(&mut self.b, *q, *p, *i),
                    Elem::D {
                        a: p,
                        b: q,
                        is,
                        nvt,
                        vcrit,
                        vd,
                    } => {
                        let raw = node_v(&self.x, *p) - node_v(&self.x, *q);
                        let lim = pnjlim(raw, *vd, *nvt, *vcrit);
                        if (lim - raw).abs() > 1e-12 {
                            limited = true;
                        }
                        *vd = lim;
                        let ex = (lim / *nvt).min(80.0).exp();
                        let id = *is * (ex - 1.0);
                        let gd = *is * ex / *nvt + GMIN;
                        let ieq = id - gd * lim;
                        stamp_g(&mut self.a, n, *p, *q, gd);
                        stamp_i(&mut self.b, *p, *q, -ieq);
                    }
                    Elem::OpAmp {
                        inp,
                        inn,
                        row,
                        gain,
                        vsat,
                        vd,
                        ..
                    } => {
                        let r = nn + *row;
                        let vsat = *vsat * rail;
                        let raw = node_v(&self.x, *inp) - node_v(&self.x, *inn);
                        // Limit the step in tanh units so Newton cannot bounce
                        // between the two saturated rails.
                        // Junction-style limiting in tanh units: inside the
                        // linear region or staying saturated on the same rail
                        // Newton is safe; otherwise land at the edge of the
                        // linear region on the new side first.
                        let to_arg = *gain / vsat;
                        let (arg_old, arg_raw) = (*vd * to_arg, raw * to_arg);
                        let vd0 = if arg_raw.abs() <= 3.0
                            || (arg_old.abs() >= 3.0 && arg_old.signum() == arg_raw.signum())
                        {
                            raw
                        } else if arg_old * arg_raw < 0.0 {
                            // Crossing over: land mid-range, where the linearization
                            // is exact, instead of ping-ponging between ±3.
                            limited = true;
                            0.0
                        } else {
                            limited = true;
                            arg_raw.clamp(-3.0, 3.0) / to_arg
                        };
                        *vd = vd0;
                        let t = (*gain * vd0 / vsat).tanh();
                        let k = *gain * (1.0 - t * t);
                        if *inp != GND {
                            self.a[r * n + *inp - 1] -= k;
                        }
                        if *inn != GND {
                            self.a[r * n + *inn - 1] += k;
                        }
                        self.b[r] = vsat * t - k * vd0;
                    }
                    Elem::R { .. } => {}
                }
            }
            if !self.solve_step() {
                // Singular matrix: keep the previous solution rather than emit NaN.
                break;
            }
            let mut delta = 0.0f64;
            for i in 0..nn {
                delta = delta.max((self.b[i] - self.x[i]).abs());
            }
            self.x.copy_from_slice(&self.b);
            // Accept after one solve when no nonlinear element moved away from
            // its linearization point; most samples then cost a single LU.
            if (!limited && (delta < TOL_V || self.nonlinear_settled())) || iters >= MAX_ITER {
                break;
            }
        }
        self.last_iters = iters;
        #[cfg(test)]
        stats::record(iters, iters >= MAX_ITER);
        // Commit capacitor state.
        for e in &mut self.elems {
            if let Elem::C {
                a: p,
                b: q,
                geq,
                j,
                v_prev,
                i_prev,
                ..
            } = e
            {
                let v = node_v(&self.x, *p) - node_v(&self.x, *q);
                *i_prev = *geq * v - *j;
                *v_prev = v;
            }
        }
    }
}

#[inline]
fn node_v(x: &[f64], n: Node) -> f64 {
    if n == GND {
        0.0
    } else {
        x[n - 1]
    }
}

#[inline]
fn stamp_g(a: &mut [f64], n: usize, p: Node, q: Node, g: f64) {
    if p != GND {
        a[(p - 1) * n + p - 1] += g;
    }
    if q != GND {
        a[(q - 1) * n + q - 1] += g;
    }
    if p != GND && q != GND {
        a[(p - 1) * n + q - 1] -= g;
        a[(q - 1) * n + p - 1] -= g;
    }
}

/// Inject current `i` into node `p` and draw it from node `q`.
#[inline]
fn stamp_i(b: &mut [f64], p: Node, q: Node, i: f64) {
    if p != GND {
        b[p - 1] += i;
    }
    if q != GND {
        b[q - 1] -= i;
    }
}

/// SPICE-style junction voltage limiting for Newton stability.
fn pnjlim(vnew: f64, vold: f64, nvt: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * nvt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / nvt;
            if arg > 0.0 {
                vold + nvt * arg.ln()
            } else {
                vcrit
            }
        } else {
            nvt * (vnew / nvt).ln()
        }
    } else {
        vnew
    }
}

/// Elimination order and structural non-zeros for a fixed row permutation.
struct LuPlan {
    /// Row `r` of the permuted system is row `perm[r]` of the original.
    perm: Vec<usize>,
    /// For pivot `k`: rows below with a (possibly filled-in) entry in column `k`.
    below: Vec<Vec<usize>>,
    /// For pivot `k`: columns right of `k` with an entry in row `k`.
    right: Vec<Vec<usize>>,
}

impl LuPlan {
    fn new(pattern: &[bool], perm: &[usize], n: usize) -> Self {
        let mut pat = vec![false; n * n];
        for r in 0..n {
            for j in 0..n {
                pat[r * n + j] = pattern[perm[r] * n + j];
            }
        }
        let mut below = vec![Vec::new(); n];
        let mut right = vec![Vec::new(); n];
        for k in 0..n {
            below[k] = ((k + 1)..n).filter(|&i| pat[i * n + k]).collect();
            right[k] = ((k + 1)..n).filter(|&j| pat[k * n + j]).collect();
            for &i in &below[k] {
                for &j in &right[k] {
                    pat[i * n + j] = true;
                }
            }
        }
        Self {
            perm: perm.to_vec(),
            below,
            right,
        }
    }

    /// Returns false (leaving `b` untouched) if a pivot is too small relative
    /// to its column, so the caller can re-pivot densely.
    fn solve(&self, a: &[f64], b: &mut [f64], lu: &mut [f64], bp: &mut [f64], n: usize) -> bool {
        for r in 0..n {
            let src = self.perm[r];
            lu[r * n..(r + 1) * n].copy_from_slice(&a[src * n..(src + 1) * n]);
            bp[r] = b[src];
        }
        for k in 0..n {
            let piv = lu[k * n + k];
            let col_max = self.below[k]
                .iter()
                .fold(0.0f64, |m, &i| m.max(lu[i * n + k].abs()));
            if !(piv.abs() > 1e-300 && piv.abs() >= 1e-3 * col_max) {
                return false;
            }
            let inv = 1.0 / piv;
            for &i in &self.below[k] {
                let f = lu[i * n + k] * inv;
                if f != 0.0 {
                    for &j in &self.right[k] {
                        lu[i * n + j] -= f * lu[k * n + j];
                    }
                    bp[i] -= f * bp[k];
                }
            }
        }
        for k in (0..n).rev() {
            let mut s = bp[k];
            for &j in &self.right[k] {
                s -= lu[k * n + j] * bp[j];
            }
            bp[k] = s / lu[k * n + k];
        }
        if !bp.iter().all(|v| v.is_finite()) {
            return false;
        }
        b.copy_from_slice(bp);
        true
    }
}

/// Dense LU solve with partial pivoting. The solution is written into `b`.
fn solve(a: &mut [f64], b: &mut [f64], perm: &mut [usize], n: usize) -> bool {
    for (i, p) in perm.iter_mut().enumerate() {
        *p = i;
    }
    for k in 0..n {
        let mut piv = k;
        let mut maxv = a[k * n + k].abs();
        for i in (k + 1)..n {
            let v = a[i * n + k].abs();
            if v > maxv {
                maxv = v;
                piv = i;
            }
        }
        if maxv < 1e-300 {
            return false;
        }
        if piv != k {
            for j in 0..n {
                a.swap(k * n + j, piv * n + j);
            }
            b.swap(k, piv);
            perm.swap(k, piv);
        }
        let inv = 1.0 / a[k * n + k];
        for i in (k + 1)..n {
            let f = a[i * n + k] * inv;
            if f != 0.0 {
                for j in (k + 1)..n {
                    a[i * n + j] -= f * a[k * n + j];
                }
                b[i] -= f * b[k];
            }
        }
    }
    for k in (0..n).rev() {
        let mut s = b[k];
        for j in (k + 1)..n {
            s -= a[k * n + j] * b[j];
        }
        b[k] = s / a[k * n + k];
    }
    b.iter().all(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_lowpass_step_response() {
        let mut cb = CircuitBuilder::new();
        let vin = cb.node();
        let out = cb.node();
        let src = cb.vsource(vin, GND);
        cb.resistor(vin, out, 1_000.0);
        cb.capacitor(out, GND, 1e-6); // tau = 1 ms
        let mut c = cb.build(48_000.0);
        c.set_voltage(src, 1.0);
        for _ in 0..48 {
            c.step();
        }
        let expected = 1.0 - (-1.0f64).exp();
        assert!((c.v(out) - expected).abs() < 1e-2, "{}", c.v(out));
    }

    #[test]
    fn diode_clamp() {
        let mut cb = CircuitBuilder::new();
        let vin = cb.node();
        let out = cb.node();
        let src = cb.vsource(vin, GND);
        cb.resistor(vin, out, 1_000.0);
        cb.diode(out, GND, 2.52e-9, 1.752, 0.02585);
        let mut c = cb.build(48_000.0);
        c.set_voltage(src, 10.0);
        c.step();
        let v = c.v(out);
        assert!(v > 0.55 && v < 0.75, "{v}");
    }

    #[test]
    fn inverting_amp() {
        let mut cb = CircuitBuilder::new();
        let vin = cb.node();
        let inv = cb.node();
        let out = cb.node();
        let src = cb.vsource(vin, GND);
        cb.resistor(vin, inv, 10_000.0);
        cb.resistor(out, inv, 20_000.0);
        cb.opamp(GND, inv, out);
        let mut c = cb.build(48_000.0);
        c.set_voltage(src, 1.0);
        c.step();
        assert!((c.v(out) + 2.0).abs() < 1e-2, "{}", c.v(out));
        // Saturates softly near the rail.
        c.set_voltage(src, 20.0);
        c.step();
        assert!(c.v(out) < -10.0 && c.v(out) > -10.6, "{}", c.v(out));
    }
}

/// Test-only Newton counters across all circuits on this thread.
#[cfg(test)]
pub mod stats {
    use std::cell::Cell;
    thread_local! {
        static S: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };
    }
    pub fn record(iters: usize, maxed: bool) {
        S.with(|s| {
            let (n, it, mx) = s.get();
            s.set((n + 1, it + iters as u64, mx + maxed as u64));
        });
    }
    /// (steps, total iterations, steps that hit MAX_ITER); resets.
    pub fn take() -> (u64, u64, u64) {
        S.with(|s| s.replace((0, 0, 0)))
    }
}
