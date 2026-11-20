//! Small modified-nodal-analysis (MNA) circuit solver.
//!
//! Capacitors use the trapezoidal rule; diodes and op-amps are solved with
//! Newton-Raphson every sample. All buffers are allocated when the circuit is
//! built, so `step` never allocates (required inside an AudioWorklet).
//!
//! Each nonlinear element changes the MNA matrix only by a rank-one term: a
//! diode adds its conductance g·(eₚ−e_q)(eₚ−e_q)ᵀ, an op-amp's linearized
//! gain k fills one row as k·e_r(e₋−e₊)ᵀ. So the matrix is factored once with
//! every nonlinear element at a nominal value (op-amps at full open-loop
//! gain, diodes at a small conductance), and each Newton iteration solves the
//! actual linearization with the Woodbury identity
//!
//!   x = y − Z (I + ΔC·W)⁻¹ ΔC·Vᵀy,   y = A₀⁻¹b, Z = A₀⁻¹U, W = VᵀZ,
//!
//! where ΔC holds each element's departure from its nominal value. Z and W
//! only change with the linear part (a knob or part edit), so an iteration
//! costs one forward/back substitution and an m×m solve (m = number of
//! diodes and op-amps, at most a handful) instead of a factorization. A
//! purely linear circuit is just a substitution. The dense pivoting solve of
//! the full matrix remains as the fallback and as the reference in tests.

pub type Node = usize;
pub const GND: Node = 0;

const GMIN: f64 = 1e-12;
const MAX_ITER: usize = 40;
const TOL_V: f64 = 1e-6;
/// Newton accepts once every diode's linearized current, at the new
/// solution, is within this of the exact one (plus 1 ppm of it) ...
const TOL_I: f64 = 1e-9;
/// ... and every op-amp's linearized output within this of the tanh curve.
const TOL_OPAMP_V: f64 = 1e-6;
const MAX_PLANS: usize = 4;
/// Nominal diode conductance folded into the factored matrix (1 MΩ): keeps
/// nodes reached only through diodes from being floating there.
const G0_DIODE: f64 = 1e-6;

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
        /// Current and conductance at `vd` (the linearization in use).
        lin: (f64, f64),
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
        /// tanh and slope at `vd` (the linearization in use).
        lin: (f64, f64),
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
            lin: (0.0, 0.0),
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
            lin: (0.0, 0.0),
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
            x_prev: vec![0.0; size],
            perm: vec![0; size],
            pattern: vec![false; size * size],
            nz: Vec::new(),
            a_stale: true,
            nl: Vec::new(),
            cval: Vec::new(),
            zmat: Vec::new(),
            wmat: Vec::new(),
            mmat: Vec::new(),
            mrhs: Vec::new(),
            s: Vec::new(),
            s_lin: Vec::new(),
            beta: Vec::new(),
            gamma: Vec::new(),
            caps: Vec::new(),
            srcs: Vec::new(),
            lowrank: false,
            plans: Vec::with_capacity(MAX_PLANS),
            rail: 1.0,
            dense_only: false,
            lu: vec![0.0; size * size],
            bp: vec![0.0; size],
            dirty: true,
            last_iters: 0,
            replans: 0,
        };
        c.nl = c
            .elems
            .iter()
            .enumerate()
            .filter_map(|(i, e)| RankOne::of(i, e, nn))
            .collect();
        c.caps = (0..c.elems.len())
            .filter(|&i| matches!(c.elems[i], Elem::C { .. }))
            .collect();
        c.srcs = (0..c.elems.len())
            .filter(|&i| matches!(c.elems[i], Elem::C { .. } | Elem::V { .. } | Elem::I { .. }))
            .collect();
        let m = c.nl.len();
        c.s = vec![0.0; m];
        c.s_lin = vec![0.0; m];
        c.beta = vec![0.0; m];
        c.gamma = vec![0.0; m];
        c.cval = vec![0.0; m];
        c.zmat = vec![0.0; size * m];
        c.wmat = vec![0.0; m * m];
        c.mmat = vec![0.0; m * m];
        c.mrhs = vec![0.0; m];
        c.build_pattern();
        c.nz = (0..size * size).filter(|&i| c.pattern[i]).collect();
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
    /// Previous sample's solution, for the predictor.
    x_prev: Vec<f64>,
    perm: Vec<usize>,
    /// Structural non-zeros of the MNA matrix (fixed by the topology).
    pattern: Vec<bool>,
    /// Dense indices of `pattern`'s entries: assembling a step copies only
    /// these from `a_lin`; every other entry of `a` stays zero.
    nz: Vec<usize>,
    /// `a` was overwritten by the dense fallback and needs a full reload.
    a_stale: bool,
    /// Nonlinear elements as rank-one terms, in element order.
    nl: Vec<RankOne>,
    /// This iteration's value (conductance or gain) of each `nl` element.
    cval: Vec<f64>,
    /// A₀⁻¹U (n×m, column-major) and VᵀA₀⁻¹U (m×m) for the factored matrix.
    zmat: Vec<f64>,
    wmat: Vec<f64>,
    /// m×m system and right-hand side scratch.
    mmat: Vec<f64>,
    mrhs: Vec<f64>,
    /// Low-rank Newton state (length m): controlling voltages, their part
    /// from the linear elements, rhs coefficients, and the solution's
    /// coefficients on Z.
    s: Vec<f64>,
    s_lin: Vec<f64>,
    beta: Vec<f64>,
    gamma: Vec<f64>,
    /// Element indices: capacitors, and every element with a right-hand side
    /// (capacitors and sources).
    caps: Vec<usize>,
    srcs: Vec<usize>,
    /// `lu` holds the factors of `a_lin` (in `plans[0]`'s order) and
    /// `zmat` / `wmat` match them.
    lowrank: bool,
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

    /// Number of non-ground nodes (nodes are numbered 1..=node_count()).
    pub fn node_count(&self) -> usize {
        self.nn
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
        // Nonlinear elements at their nominal values.
        for t in &self.nl {
            t.stamp(a, n, t.c0);
        }
        self.dirty = false;
        self.lowrank = false;
    }

    /// Factor `a_lin` and precompute Z and W. Leaves `lowrank` false (so the
    /// full solve is used) if the nominal matrix cannot be factored.
    fn setup_lowrank(&mut self) {
        let n = self.size;
        let m = self.nl.len();
        self.a.copy_from_slice(&self.a_lin);
        self.a_stale = false;
        let mut ok = self
            .plans
            .first()
            .is_some_and(|p| p.factor(&self.a, &mut self.lu, n));
        if !ok {
            // Pivot order for the nominal matrix from dense partial pivoting.
            self.b.iter_mut().for_each(|v| *v = 0.0);
            if !solve(&mut self.a, &mut self.b, &mut self.perm, n) {
                return;
            }
            self.a.copy_from_slice(&self.a_lin);
            self.replans += 1;
            if self.plans.len() == MAX_PLANS {
                self.plans.pop();
            }
            self.plans
                .insert(0, LuPlan::new(&self.pattern, &self.perm, n));
            ok = self.plans[0].factor(&self.a, &mut self.lu, n);
        }
        if !ok {
            return;
        }
        for j in 0..m {
            let col = &mut self.zmat[j * n..(j + 1) * n];
            col.iter_mut().for_each(|v| *v = 0.0);
            for &(i, sgn) in self.nl[j].u() {
                col[i] = sgn;
            }
            if !self.plans[0].resubstitute(col, &self.lu, &mut self.bp, n) {
                return;
            }
        }
        for i in 0..m {
            for j in 0..m {
                self.wmat[i * m + j] = self.nl[i].vdot(&self.zmat[j * n..(j + 1) * n]);
            }
        }
        self.lowrank = true;
    }

    /// Assemble the full linearized matrix into `a` (fallback and tests).
    fn assemble_full(&mut self) {
        let n = self.size;
        if self.a_stale {
            self.a.copy_from_slice(&self.a_lin);
            self.a_stale = false;
        } else {
            for &i in &self.nz {
                self.a[i] = self.a_lin[i];
            }
        }
        for (t, &c) in self.nl.iter().zip(&self.cval) {
            t.stamp(&mut self.a, n, c - t.c0);
        }
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
                    // plans[0] no longer matches the factors in `lu`.
                    self.lowrank = false;
                }
                return true;
            }
        }
        self.a_stale = true;
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

    /// Whether every nonlinear element's linearization, evaluated at the new
    /// solution, matches its exact curve within tolerance (so another Newton
    /// iteration would not move the result measurably).
    fn nonlinear_settled(&self) -> bool {
        let rail = self.rail;
        self.nl.iter().all(|t| match self.elems[t.elem] {
            Elem::D {
                a,
                b,
                is,
                nvt,
                vd,
                lin,
                ..
            } => diode_settled(node_v(&self.x, a) - node_v(&self.x, b), is, nvt, vd, lin),
            Elem::OpAmp {
                inp,
                inn,
                gain,
                vsat,
                vd,
                lin,
                ..
            } => opamp_settled(
                node_v(&self.x, inp) - node_v(&self.x, inn),
                gain,
                vsat * rail,
                vd,
                lin,
            ),
            _ => true,
        })
    }

    /// Advance the circuit by one sample.
    pub fn step(&mut self) {
        if self.dirty {
            self.rebuild_linear();
        }
        let dense = self.dense_only || test_force_dense();
        if !self.lowrank && !dense {
            self.setup_lowrank();
        }
        // Capacitor history currents for this step.
        for &i in &self.caps {
            if let Elem::C {
                geq,
                j,
                v_prev,
                i_prev,
                ..
            } = &mut self.elems[i]
            {
                *j = *geq * *v_prev + *i_prev;
            }
        }
        // Linearize the first iteration at the solution extrapolated from the
        // last two samples rather than at the last one: the nonlinear elements
        // then usually land within tolerance after a single solve.
        for (x, xp) in self.x.iter_mut().zip(self.x_prev.iter_mut()) {
            let now = *x;
            *x = 2.0 * now - *xp;
            *xp = now;
        }
        let low = if self.lowrank && !dense {
            self.newton_lowrank()
        } else {
            None
        };
        let iters = match low {
            Some(it) => it,
            None => self.newton_full(),
        };
        self.last_iters = iters;
        #[cfg(test)]
        stats::record(iters, iters >= MAX_ITER);
        // Commit capacitor state.
        for &i in &self.caps {
            if let Elem::C {
                a: p,
                b: q,
                geq,
                j,
                v_prev,
                i_prev,
                ..
            } = &mut self.elems[i]
            {
                let v = node_v(&self.x, *p) - node_v(&self.x, *q);
                *i_prev = *geq * v - *j;
                *v_prev = v;
            }
        }
    }

    /// Right-hand side of the linear elements (capacitor history, sources).
    fn linear_rhs(&mut self) {
        let nn = self.nn;
        self.b.iter_mut().for_each(|v| *v = 0.0);
        for &i in &self.srcs {
            match self.elems[i] {
                Elem::C { a: p, b: q, j, .. } => stamp_i(&mut self.b, p, q, j),
                Elem::V { v, row, .. } => self.b[nn + row] = v,
                Elem::I { a: p, b: q, i } => stamp_i(&mut self.b, q, p, i),
                _ => {}
            }
        }
    }

    /// Linearize nonlinear element `j` of `nl` at controlling voltage `raw`.
    /// Sets `cval[j]` and returns (limited, rhs coefficient on its u column).
    fn linearize(&mut self, j: usize, raw: f64) -> (bool, f64) {
        let rail = self.rail;
        match &mut self.elems[self.nl[j].elem] {
            Elem::D {
                is,
                nvt,
                vcrit,
                vd,
                lin,
                ..
            } => {
                let (limited, id, gd) = lin_diode(raw, vd, *is, *nvt, *vcrit);
                *lin = (id, gd);
                self.cval[j] = gd;
                // The companion current source −(id − gd·vd) flows a → b.
                (limited, -(id - gd * *vd))
            }
            Elem::OpAmp {
                gain,
                vsat,
                vd,
                lin,
                ..
            } => {
                let vsat = *vsat * rail;
                let (limited, t, k) = lin_opamp(raw, vd, *gain, vsat);
                *lin = (t, k);
                self.cval[j] = k;
                (limited, vsat * t - k * *vd)
            }
            _ => (false, 0.0),
        }
    }

    /// Newton on the nonlinear elements' m controlling voltages only, from
    /// the factored nominal matrix (see the module doc). Within one sample,
    /// x = y + Z·γ where y = A₀⁻¹b_lin is fixed, so an iteration costs O(m²);
    /// x itself is formed once at the end. None if the low-rank solve breaks
    /// down (the caller falls back to the full solve).
    fn newton_lowrank(&mut self) -> Option<usize> {
        let n = self.size;
        let m = self.nl.len();
        self.linear_rhs();
        if !self.plans[0].resubstitute(&mut self.b, &self.lu, &mut self.bp, n) {
            return None;
        }
        if m == 0 {
            self.x.copy_from_slice(&self.b);
            return Some(1);
        }
        for j in 0..m {
            self.s_lin[j] = self.nl[j].vdot(&self.b);
            self.s[j] = self.nl[j].vdot(&self.x);
        }
        let mut iters = 0;
        loop {
            iters += 1;
            let mut limited = false;
            for j in 0..m {
                let raw = self.nl[j].vsign * self.s[j];
                let (lim, beta) = self.linearize(j, raw);
                limited |= lim;
                self.beta[j] = beta;
            }
            // (I + ΔC·W) z = ΔC·Vᵀy with Vᵀy = s_lin + W·β; then γ = β − z
            // and the new controlling voltages are s_lin + W·γ.
            for i in 0..m {
                let dc = self.cval[i] - self.nl[i].c0;
                let row = &self.wmat[i * m..(i + 1) * m];
                let q = self.s_lin[i] + dot(row, &self.beta);
                for (j, w) in row.iter().enumerate() {
                    self.mmat[i * m + j] = dc * w + if i == j { 1.0 } else { 0.0 };
                }
                self.mrhs[i] = dc * q;
            }
            if !small_solve(&mut self.mmat, &mut self.mrhs, m) {
                return None;
            }
            for j in 0..m {
                self.gamma[j] = self.beta[j] - self.mrhs[j];
            }
            for i in 0..m {
                self.s[i] = self.s_lin[i] + dot(&self.wmat[i * m..(i + 1) * m], &self.gamma);
            }
            if iters >= MAX_ITER || (!limited && self.lowrank_settled()) {
                break;
            }
        }
        for j in 0..m {
            let g = self.gamma[j];
            if g != 0.0 {
                for (y, z) in self.b.iter_mut().zip(&self.zmat[j * n..(j + 1) * n]) {
                    *y += z * g;
                }
            }
        }
        if !self.b.iter().all(|v| v.is_finite()) {
            return None;
        }
        self.x.copy_from_slice(&self.b);
        Some(iters)
    }

    /// `nonlinear_settled` on the controlling voltages `s`.
    fn lowrank_settled(&self) -> bool {
        let rail = self.rail;
        self.nl.iter().zip(&self.s).all(|(t, &s)| {
            let v = t.vsign * s;
            match self.elems[t.elem] {
                Elem::D {
                    is, nvt, vd, lin, ..
                } => diode_settled(v, is, nvt, vd, lin),
                Elem::OpAmp {
                    gain,
                    vsat,
                    vd,
                    lin,
                    ..
                } => opamp_settled(v, gain, vsat * rail, vd, lin),
                _ => true,
            }
        })
    }

    /// Newton on the fully assembled matrix: the reference path (tests) and
    /// the fallback when the nominal matrix cannot be factored.
    fn newton_full(&mut self) -> usize {
        let nn = self.nn;
        let m = self.nl.len();
        let mut iters = 0;
        loop {
            iters += 1;
            self.linear_rhs();
            let mut limited = false;
            for j in 0..m {
                let raw = self.nl[j].vdot(&self.x) * self.nl[j].vsign;
                let (lim, beta) = self.linearize(j, raw);
                limited |= lim;
                for &(i, sgn) in self.nl[j].u() {
                    self.b[i] += sgn * beta;
                }
            }
            self.assemble_full();
            if !self.solve_step() {
                // Singular matrix: keep the previous solution rather than emit NaN.
                if iters == 1 {
                    self.x.copy_from_slice(&self.x_prev);
                }
                break;
            }
            let mut delta = 0.0f64;
            for i in 0..nn {
                delta = delta.max((self.b[i] - self.x[i]).abs());
            }
            self.x.copy_from_slice(&self.b);
            if (!limited && (delta < TOL_V || self.nonlinear_settled())) || iters >= MAX_ITER {
                break;
            }
        }
        iters
    }
}

/// Diode linearization at `raw` with SPICE junction limiting against the
/// previous point `vd` (updated). Returns (limited, current, conductance).
fn lin_diode(raw: f64, vd: &mut f64, is: f64, nvt: f64, vcrit: f64) -> (bool, f64, f64) {
    let lim = pnjlim(raw, *vd, nvt, vcrit);
    *vd = lim;
    let ex = (lim / nvt).min(80.0).exp();
    (
        (lim - raw).abs() > 1e-12,
        is * (ex - 1.0),
        is * ex / nvt + GMIN,
    )
}

/// Op-amp linearization at differential input `raw` against the previous
/// point `vd` (updated). Returns (limited, tanh, slope).
fn lin_opamp(raw: f64, vd: &mut f64, gain: f64, vsat: f64) -> (bool, f64, f64) {
    // Junction-style limiting in tanh units: inside the linear region or
    // staying saturated on the same rail Newton is safe; otherwise land at
    // the edge of the linear region on the new side first, so Newton cannot
    // bounce between the two saturated rails.
    let to_arg = gain / vsat;
    let (arg_old, arg_raw) = (*vd * to_arg, raw * to_arg);
    let (limited, vd0) =
        if arg_raw.abs() <= 3.0 || (arg_old.abs() >= 3.0 && arg_old.signum() == arg_raw.signum()) {
            (false, raw)
        } else if arg_old * arg_raw < 0.0 {
            // Crossing over: land mid-range, where the linearization is exact,
            // instead of ping-ponging between ±3.
            (true, 0.0)
        } else {
            (true, arg_raw.clamp(-3.0, 3.0) / to_arg)
        };
    *vd = vd0;
    let t = (gain * vd0 / vsat).tanh();
    (limited, t, gain * (1.0 - t * t))
}

/// The diode's linearized current at `v` is within tolerance of the exact one.
fn diode_settled(v: f64, is: f64, nvt: f64, vd: f64, (id, gd): (f64, f64)) -> bool {
    let exact = is * ((v / nvt).min(80.0).exp() - 1.0);
    (exact - (id + gd * (v - vd))).abs() <= TOL_I + 1e-6 * exact.abs()
}

/// The op-amp's linearized output at `v` is within tolerance of the curve.
fn opamp_settled(v: f64, gain: f64, vsat: f64, vd: f64, (t, k): (f64, f64)) -> bool {
    let exact = vsat * (gain * v / vsat).tanh();
    (exact - (vsat * t + k * (v - vd))).abs() <= TOL_OPAMP_V
}

#[inline]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A nonlinear element as the rank-one matrix term c·u·vᵀ (matrix indices).
struct RankOne {
    /// Index in `elems`.
    elem: usize,
    /// vᵀx times this is the element's controlling voltage (diode
    /// anode−cathode, op-amp + minus −).
    vsign: f64,
    u: [(usize, f64); 2],
    nu: usize,
    v: [(usize, f64); 2],
    nv: usize,
    /// Nominal value folded into the factored matrix.
    c0: f64,
}

impl RankOne {
    fn of(elem: usize, e: &Elem, nn: usize) -> Option<Self> {
        let pair = |p: Node, q: Node| {
            let mut t = [(0, 0.0); 2];
            let mut k = 0;
            for (nd, sgn) in [(p, 1.0), (q, -1.0)] {
                if nd != GND {
                    t[k] = (nd - 1, sgn);
                    k += 1;
                }
            }
            (t, k)
        };
        match *e {
            Elem::D { a, b, .. } => {
                let (u, nu) = pair(a, b);
                Some(Self {
                    elem,
                    vsign: 1.0,
                    u,
                    nu,
                    v: u,
                    nv: nu,
                    c0: G0_DIODE,
                })
            }
            Elem::OpAmp {
                inp,
                inn,
                row,
                gain,
                ..
            } => {
                let (v, nv) = pair(inn, inp);
                Some(Self {
                    elem,
                    vsign: -1.0,
                    u: [(nn + row, 1.0), (0, 0.0)],
                    nu: 1,
                    v,
                    nv,
                    c0: gain,
                })
            }
            _ => None,
        }
    }

    fn u(&self) -> &[(usize, f64)] {
        &self.u[..self.nu]
    }

    fn vdot(&self, x: &[f64]) -> f64 {
        self.v[..self.nv].iter().map(|&(i, s)| s * x[i]).sum()
    }

    /// a += c·u·vᵀ
    fn stamp(&self, a: &mut [f64], n: usize, c: f64) {
        for &(i, su) in self.u() {
            for &(j, sv) in &self.v[..self.nv] {
                a[i * n + j] += c * su * sv;
            }
        }
    }
}

/// Gaussian elimination with partial pivoting on a small dense m×m system
/// (row-major `a`); the solution replaces `b`.
fn small_solve(a: &mut [f64], b: &mut [f64], m: usize) -> bool {
    for k in 0..m {
        let mut piv = k;
        for i in (k + 1)..m {
            if a[i * m + k].abs() > a[piv * m + k].abs() {
                piv = i;
            }
        }
        if a[piv * m + k].abs() < 1e-300 {
            return false;
        }
        if piv != k {
            for j in 0..m {
                a.swap(k * m + j, piv * m + j);
            }
            b.swap(k, piv);
        }
        let inv = 1.0 / a[k * m + k];
        for i in (k + 1)..m {
            let f = a[i * m + k] * inv;
            if f != 0.0 {
                for j in (k + 1)..m {
                    a[i * m + j] -= f * a[k * m + j];
                }
                b[i] -= f * b[k];
            }
        }
    }
    for k in (0..m).rev() {
        let mut s = b[k];
        for j in (k + 1)..m {
            s -= a[k * m + j] * b[j];
        }
        b[k] = s / a[k * m + k];
    }
    b.iter().all(|v| v.is_finite())
}

/// Tests can force every circuit onto the dense full solve to compare.
#[cfg(test)]
fn test_force_dense() -> bool {
    stats::FORCE_DENSE.with(|f| f.get())
}
#[cfg(not(test))]
#[inline]
fn test_force_dense() -> bool {
    false
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

/// Elimination plan for a fixed row permutation, compiled into flat index
/// lists over the structural non-zeros (fill-in included) of the permuted
/// matrix. A solve touches only those entries, in compact storage, so its
/// cost follows the number of non-zeros instead of n².
struct LuPlan {
    /// Row `r` of the permuted system is row `perm[r]` of the original.
    perm: Vec<usize>,
    /// (slot, dense index into the unpermuted matrix) of each structural entry.
    load: Vec<(u32, u32)>,
    /// Slots that only appear as fill-in; zeroed before each factorization.
    fill: Vec<u32>,
    /// Slot of each pivot.
    diag: Vec<u32>,
    /// Per pivot: range of `rows`.
    rows_at: Vec<(u32, u32)>,
    /// Rows below a pivot with an entry in its column:
    /// (slot of that entry, row, range of `pairs`).
    rows: Vec<(u32, u32, u32, u32)>,
    /// Row updates: (slot (i, j), slot (k, j)) for j right of pivot k.
    pairs: Vec<(u32, u32)>,
    /// Per pivot: range of `back`.
    back_at: Vec<(u32, u32)>,
    /// Back substitution terms: (slot (k, j), j).
    back: Vec<(u32, u32)>,
    /// Forward substitution in pivot order: (slot (i, k), i, k).
    fwd: Vec<(u32, u32, u32)>,
    /// Number of slots (`lu` must be at least this long).
    slots: usize,
}

impl LuPlan {
    fn new(pattern: &[bool], perm: &[usize], n: usize) -> Self {
        let mut pat = vec![false; n * n];
        for r in 0..n {
            for j in 0..n {
                pat[r * n + j] = pattern[perm[r] * n + j];
            }
        }
        let orig = pat.clone();
        // Symbolic elimination: which entries fill in.
        let mut below = vec![Vec::new(); n];
        let mut right = vec![Vec::new(); n];
        for k in 0..n {
            pat[k * n + k] = true;
            below[k] = ((k + 1)..n).filter(|&i| pat[i * n + k]).collect::<Vec<_>>();
            right[k] = ((k + 1)..n).filter(|&j| pat[k * n + j]).collect::<Vec<_>>();
            for &i in &below[k] {
                for &j in &right[k] {
                    pat[i * n + j] = true;
                }
            }
        }
        let mut slot = vec![u32::MAX; n * n];
        let (mut load, mut fill) = (Vec::new(), Vec::new());
        let mut count = 0u32;
        for r in 0..n {
            for j in 0..n {
                if pat[r * n + j] {
                    slot[r * n + j] = count;
                    if orig[r * n + j] {
                        load.push((count, (perm[r] * n + j) as u32));
                    } else {
                        fill.push(count);
                    }
                    count += 1;
                }
            }
        }
        let at = |i: usize, j: usize| slot[i * n + j];
        let mut plan = Self {
            perm: perm.to_vec(),
            load,
            fill,
            diag: (0..n).map(|k| at(k, k)).collect(),
            rows_at: Vec::with_capacity(n),
            rows: Vec::new(),
            pairs: Vec::new(),
            back_at: Vec::with_capacity(n),
            back: Vec::new(),
            fwd: Vec::new(),
            slots: count as usize,
        };
        for k in 0..n {
            let r0 = plan.rows.len() as u32;
            for &i in &below[k] {
                let p0 = plan.pairs.len() as u32;
                plan.pairs
                    .extend(right[k].iter().map(|&j| (at(i, j), at(k, j))));
                plan.fwd.push((at(i, k), i as u32, k as u32));
                plan.rows
                    .push((at(i, k), i as u32, p0, plan.pairs.len() as u32));
            }
            plan.rows_at.push((r0, plan.rows.len() as u32));
            let b0 = plan.back.len() as u32;
            plan.back
                .extend(right[k].iter().map(|&j| (at(k, j), j as u32)));
            plan.back_at.push((b0, plan.back.len() as u32));
        }
        // `resubstitute` indexes without bounds checks; everything it reads
        // comes from here, so check it once.
        let slots = plan.slots as u32;
        assert!(plan.perm.iter().all(|&r| r < n));
        assert!(plan.diag.iter().all(|&d| d < slots));
        assert!(plan
            .fwd
            .iter()
            .all(|&(s, i, k)| s < slots && (i as usize) < n && (k as usize) < n));
        assert!(plan
            .back
            .iter()
            .all(|&(s, j)| s < slots && (j as usize) < n));
        assert!(
            plan.back_at.len() == n
                && plan
                    .back_at
                    .iter()
                    .all(|&(b0, b1)| b0 <= b1 && b1 as usize <= plan.back.len())
        );
        plan
    }

    /// Factor `a` and solve for `b` (solution into `b`). Returns false
    /// (leaving `b` untouched) if a pivot is too small relative to its column,
    /// so the caller can re-pivot densely.
    fn solve(&self, a: &[f64], b: &mut [f64], lu: &mut [f64], bp: &mut [f64], n: usize) -> bool {
        self.factor(a, lu, n) && self.resubstitute(b, lu, bp, n)
    }

    /// Factor `a` into `lu` (multipliers below the diagonal). Returns false
    /// if a pivot is too small relative to its column.
    fn factor(&self, a: &[f64], lu: &mut [f64], n: usize) -> bool {
        for &(s, src) in &self.load {
            lu[s as usize] = a[src as usize];
        }
        for &s in &self.fill {
            lu[s as usize] = 0.0;
        }
        for k in 0..n {
            let piv = lu[self.diag[k] as usize];
            let (r0, r1) = self.rows_at[k];
            let rows = &self.rows[r0 as usize..r1 as usize];
            let col_max = rows
                .iter()
                .fold(0.0f64, |m, r| m.max(lu[r.0 as usize].abs()));
            if !(piv.abs() > 1e-300 && piv.abs() >= 1e-3 * col_max) {
                return false;
            }
            let inv = 1.0 / piv;
            for &(ik, _, p0, p1) in rows {
                let f = lu[ik as usize] * inv;
                lu[ik as usize] = f;
                if f != 0.0 {
                    for &(ij, kj) in &self.pairs[p0 as usize..p1 as usize] {
                        lu[ij as usize] -= f * lu[kj as usize];
                    }
                }
            }
        }
        true
    }

    /// Solve for `b` (solution into `b`) with the factors in `lu`; false
    /// (leaving `b` untouched) if the result is not finite.
    fn resubstitute(&self, b: &mut [f64], lu: &[f64], bp: &mut [f64], n: usize) -> bool {
        // This runs once per circuit per sample, so it skips bounds checks.
        assert!(n == self.perm.len() && b.len() >= n && bp.len() >= n && lu.len() >= self.slots);
        // SAFETY: the lengths were just checked, and `LuPlan::new` asserted
        // that every row index is < n and every slot < `slots`.
        unsafe {
            for r in 0..n {
                *bp.get_unchecked_mut(r) = *b.get_unchecked(*self.perm.get_unchecked(r));
            }
            for &(ik, i, k) in &self.fwd {
                let bk = *bp.get_unchecked(k as usize);
                *bp.get_unchecked_mut(i as usize) -= lu.get_unchecked(ik as usize) * bk;
            }
            for k in (0..n).rev() {
                let (b0, b1) = *self.back_at.get_unchecked(k);
                let mut s = *bp.get_unchecked(k);
                for &(kj, j) in self.back.get_unchecked(b0 as usize..b1 as usize) {
                    s -= lu.get_unchecked(kj as usize) * bp.get_unchecked(j as usize);
                }
                *bp.get_unchecked_mut(k) =
                    s / lu.get_unchecked(*self.diag.get_unchecked(k) as usize);
            }
        }
        if !bp[..n].iter().all(|v| v.is_finite()) {
            return false;
        }
        b[..n].copy_from_slice(&bp[..n]);
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
        pub static FORCE_DENSE: Cell<bool> = const { Cell::new(false) };
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
