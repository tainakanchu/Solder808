//! Component values with per-unit tolerance, aging and temperature.
//!
//! A "unit" is one imaginary physical machine: its seed decides how far each
//! part deviates from nominal, `age_years` drifts parts the way old components
//! do, and `temp_c` moves semiconductor and resistor values with temperature.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PartKind {
    /// Carbon film resistor, ±5 %, drifts upward with age.
    Carbon = 0,
    /// Polyester film capacitor, ±10 %, very stable.
    FilmCap = 1,
    /// Ceramic disc capacitor, ±20 %, temperature sensitive.
    Ceramic = 2,
    /// Aluminium electrolytic capacitor, ±20 %, loses capacitance with age.
    Electrolytic = 3,
    /// Silicon small-signal diode; the value is the saturation current Is.
    Diode = 4,
    /// Trimmer / panel potentiometer track, ±20 %.
    Pot = 5,
}

#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub seed: u32,
    pub age_years: f64,
    pub temp_c: f64,
}

impl Default for Unit {
    fn default() -> Self {
        Self {
            seed: 808,
            age_years: 0.0,
            temp_c: 25.0,
        }
    }
}

impl Unit {
    /// Thermal voltage kT/q.
    pub fn vt(&self) -> f64 {
        8.617_333e-5 * (self.temp_c + 273.15)
    }
}

#[derive(Clone, Debug)]
pub struct Part {
    pub name: &'static str,
    pub kind: PartKind,
    /// Nominal value (ohms, farads, or amps for diodes). For pots this is the
    /// resistance at the current knob position.
    pub nominal: f64,
    /// Manual override multiplier set from the UI (1.0 = untouched).
    pub user_scale: f64,
    /// Value after tolerance, aging, temperature and override.
    pub effective: f64,
    /// Diode emission coefficient after tolerance (unused for other kinds).
    pub n: f64,
}

impl Part {
    pub fn new(name: &'static str, kind: PartKind, nominal: f64) -> Self {
        Self {
            name,
            kind,
            nominal,
            user_scale: 1.0,
            effective: nominal,
            n: 1.752,
        }
    }
}

/// Recompute effective values for every part of a voice.
pub fn apply_unit(parts: &mut [Part], unit: &Unit, voice_id: u32) {
    let y = unit.age_years.max(0.0);
    let dt = unit.temp_c - 25.0;
    for (i, p) in parts.iter_mut().enumerate() {
        let z = gauss(unit.seed, voice_id, i as u32, 0);
        let z_age = gauss(unit.seed, voice_id, i as u32, 1);
        let f = match p.kind {
            PartKind::Carbon => {
                (1.0 + 0.025 * z) * (1.0 + y * (0.0012 + 0.0006 * z_age)) * (1.0 - 5e-4 * dt)
            }
            PartKind::FilmCap => (1.0 + 0.05 * z) * (1.0 - 0.0004 * y),
            PartKind::Ceramic => (1.0 + 0.08 * z) * (1.0 - 0.001 * y) * (1.0 - 1.5e-3 * dt),
            PartKind::Electrolytic => {
                let loss = y * (0.008 + 0.003 * z_age).max(0.002);
                (1.0 + 0.1 * z) * (1.0 - loss).max(0.25)
            }
            PartKind::Diode => {
                p.n = 1.752 * (1.0 + 0.02 * z_age);
                (0.4 * z).exp() * (1.0 + 0.004 * y) * 2f64.powf(dt / 8.0)
            }
            PartKind::Pot => (1.0 + 0.1 * z) * (1.0 + 0.001 * y),
        };
        p.effective = p.nominal * f * p.user_scale;
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Deterministic standard-normal sample, clamped to ±2.5σ.
fn gauss(seed: u32, voice: u32, part: u32, stream: u32) -> f64 {
    let key = ((seed as u64) << 32) ^ ((voice as u64) << 20) ^ ((part as u64) << 4) ^ stream as u64;
    let a = splitmix(key);
    let b = splitmix(a);
    let u1 = ((a >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    let u2 = (b >> 11) as f64 / (1u64 << 53) as f64;
    let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    z.clamp(-2.5, 2.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_unit() {
        let mut a = vec![Part::new("R1", PartKind::Carbon, 1000.0)];
        let mut b = a.clone();
        let u = Unit {
            seed: 42,
            ..Default::default()
        };
        apply_unit(&mut a, &u, 0);
        apply_unit(&mut b, &u, 0);
        assert_eq!(a[0].effective, b[0].effective);
        let u2 = Unit {
            seed: 43,
            ..Default::default()
        };
        apply_unit(&mut b, &u2, 0);
        assert_ne!(a[0].effective, b[0].effective);
    }

    #[test]
    fn electrolytics_lose_capacitance() {
        let mut p = vec![Part::new("C1", PartKind::Electrolytic, 1e-6)];
        apply_unit(&mut p, &Unit::default(), 0);
        let new = p[0].effective;
        apply_unit(
            &mut p,
            &Unit {
                age_years: 45.0,
                ..Default::default()
            },
            0,
        );
        assert!(p[0].effective < new * 0.85);
    }
}
