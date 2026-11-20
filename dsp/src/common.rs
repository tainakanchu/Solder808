//! Circuitry shared by all voices: the power supply rail, the accent line and
//! the output mixer.
//!
//! - Supply: a 15 V source with output resistance and a filter capacitor. The
//!   voices' current draw sags the rail, which scales every op-amp's swing and
//!   the trigger levels.
//! - Accent line: one source resistance feeding every triggered voice's input
//!   resistance, so the more voices fire together, the lower the accent.
//! - Mixer: level pots (dividers) → mix resistors → inverting summing op-amp →
//!   output coupling capacitor, solved as a circuit at the oversampled rate.

use crate::blocks::PartMap;
use crate::circuit::{Circuit, CircuitBuilder, ElemId, Node, GND};
use crate::engine::VOICES;
use crate::parts::{apply_unit, Part, PartKind as K, Unit};
use crate::voice::{audio_taper, Bus};

/// Tolerance seed salt. It predates the toms (voice 5) and is kept so
/// existing units keep their values; the new voices use salts 11..15.
const VOICE_ID: u32 = 5;
const RAIL_NOMINAL: f64 = 15.0;
/// Voice output (after trim) → mixer input volts.
const MIX_IN: f64 = 3.0;
const POT_NAMES: [&str; VOICES] = [
    "VR1 BD level",
    "VR2 SD level",
    "VR3 HH level",
    "VR4 CP/MA level",
    "VR5 CB level",
    "VR6 LT/LC level",
    "VR7 MT/MC level",
    "VR8 HT/HC level",
    "VR9 RS/CL level",
    "VR10 CY level",
];
const MIX_NAMES: [&str; VOICES] = [
    "R4 BD mix",
    "R5 SD mix",
    "R6 HH mix",
    "R7 CP/MA mix",
    "R8 CB mix",
    "R11 LT/LC mix",
    "R12 MT/MC mix",
    "R13 HT/HC mix",
    "R14 RS/CL mix",
    "R15 CY mix",
];
/// Voices whose mixer legs come before the mixer's own parts in the table
/// (the original five); later voices' legs are appended at the end so the
/// existing part indices, and therefore every unit's tolerance draws, stay put.
const FIRST_VOICES: usize = 5;

pub struct Common {
    pm: PartMap,
    mix: Circuit,
    srcs: [ElemId; VOICES],
    legs: [ElemId; VOICES],
    pots: [usize; VOICES],
    mixes: [usize; VOICES],
    /// Per voice: divider ratio and Norton conductance of pot + mix resistor.
    ratio: [f64; VOICES],
    g_leg: [f64; VOICES],
    levels: [f64; VOICES],
    out: Node,
    r_supply: usize,
    c_supply: usize,
    r_acc_src: usize,
    r_acc_in: usize,
    rail_v: f64,
    dt: f64,
    /// Accent knob, 0..1 (0.8 = the unloaded level used before this knob existed).
    pub accent_knob: f64,
}

impl Common {
    pub fn new(sr: f64) -> Self {
        let mut pm = PartMap::default();
        let r_supply = pm.add("R1 supply output", K::Carbon, 10.0);
        let c_supply = pm.add("C1 supply filter", K::Electrolytic, 470e-6);
        let r_acc_src = pm.add("R2 accent source", K::Carbon, 2.2e3);
        let r_acc_in = pm.add("R3 accent input (each voice)", K::Carbon, 22e3);

        // Each level pot + mix resistor is reduced to its Norton equivalent
        // into the summing node (exact for a linear divider), which keeps the
        // mixer to four unknowns.
        let mut cb = CircuitBuilder::new();
        let inv = cb.node();
        let out = cb.node();
        let oc = cb.node();
        let mut srcs = Vec::new();
        let mut legs = Vec::new();
        let mut pots = Vec::new();
        let mut mixes = Vec::new();
        let mut leg = |cb: &mut CircuitBuilder, pm: &mut PartMap, k: usize| {
            srcs.push(cb.isource(GND, inv));
            legs.push(cb.resistor(inv, GND, 1.0));
            pots.push(pm.add(POT_NAMES[k], K::Pot, 10e3));
            mixes.push(pm.add(MIX_NAMES[k], K::Carbon, 47e3));
        };
        for k in 0..FIRST_VOICES {
            leg(&mut cb, &mut pm, k);
        }
        cb.opamp(GND, inv, out);
        pm.r(&mut cb, out, inv, "R9 mixer feedback", 47e3);
        pm.c(
            &mut cb,
            out,
            inv,
            "C2 mixer HF roll-off",
            K::Ceramic,
            100e-12,
        );
        pm.c(&mut cb, out, oc, "C3 output", K::Electrolytic, 10e-6);
        pm.r(&mut cb, oc, GND, "R10 output load", 10e3);
        for k in FIRST_VOICES..VOICES {
            leg(&mut cb, &mut pm, k);
        }

        let to_arr = |v: Vec<ElemId>| -> [ElemId; VOICES] {
            v.try_into().unwrap_or_else(|_| unreachable!())
        };
        let mut c = Self {
            mix: cb.build(sr),
            pm,
            srcs: to_arr(srcs),
            legs: to_arr(legs),
            pots: pots.try_into().unwrap_or_else(|_| unreachable!()),
            mixes: mixes.try_into().unwrap_or_else(|_| unreachable!()),
            ratio: [0.0; VOICES],
            g_leg: [0.0; VOICES],
            levels: [0.8; VOICES],
            out: oc,
            r_supply,
            c_supply,
            r_acc_src,
            r_acc_in,
            rail_v: RAIL_NOMINAL,
            dt: 1.0 / sr,
            accent_knob: 0.8,
        };
        c.refresh(&Unit::default());
        c
    }

    pub fn parts(&self) -> &[Part] {
        &self.pm.parts
    }

    pub fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.pm.parts
    }

    pub fn refresh(&mut self, unit: &Unit) {
        apply_unit(&mut self.pm.parts, unit, VOICE_ID);
        self.pm.push(&mut self.mix, unit);
        self.push_pots();
    }

    pub fn set_level(&mut self, voice: usize, value: f64) {
        if voice < VOICES {
            self.levels[voice] = value.clamp(0.0, 1.0);
            self.push_pots();
        }
    }

    fn push_pots(&mut self) {
        for k in 0..VOICES {
            let r = self.pm.parts[self.pots[k]].effective;
            let p = audio_taper(self.levels[k]);
            // Thevenin of the divider: v·p behind r·p·(1−p), then the mix resistor.
            let r_th = r * p * (1.0 - p) + self.pm.parts[self.mixes[k]].effective;
            self.ratio[k] = p;
            self.g_leg[k] = 1.0 / r_th;
            self.mix.set_resistance(self.legs[k], r_th);
        }
    }

    /// Bus state given the accent-weighted number of voices loading the line.
    pub fn bus(&self, accent_draw: f64) -> Bus {
        let p = |i: usize| self.pm.parts[i].effective;
        let loading = 1.0 / (1.0 + p(self.r_acc_src) * accent_draw / p(self.r_acc_in));
        Bus {
            accent: self.accent_knob * 1.25 * loading,
            rail: self.rail_v / RAIL_NOMINAL,
            metal: 0.0,
        }
    }

    pub fn rail_volts(&self) -> f64 {
        self.rail_v
    }

    /// Advance the supply filter by one sample given the total load current.
    pub fn update_rail(&mut self, current: f64) {
        let r = self.pm.parts[self.r_supply].effective;
        let c = self.pm.parts[self.c_supply].effective;
        let dv = ((RAIL_NOMINAL - self.rail_v) / r - current) / c * self.dt;
        self.rail_v = (self.rail_v + dv).clamp(RAIL_NOMINAL * 0.5, RAIL_NOMINAL);
    }

    /// Mix one oversampled frame of voice outputs (already trimmed to ~±1).
    pub fn mix(&mut self, outs: &[f64; VOICES], any_active: bool, rail: f64) -> f64 {
        if !any_active && self.mix.energy() < 1e-6 {
            return 0.0;
        }
        for (k, &out) in outs.iter().enumerate() {
            self.mix.set_current(
                self.srcs[k],
                out * MIX_IN * self.ratio[k] * self.g_leg[k],
            );
        }
        self.mix.rail = rail;
        self.mix.step();
        // Inverting stage: flip back and undo the input scaling.
        -self.mix.v(self.out) / MIX_IN
    }
}
