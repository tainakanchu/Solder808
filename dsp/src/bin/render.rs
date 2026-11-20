//! Native check: render single hits to WAV and print simple measurements.
//! Usage: cargo run --release --bin render -- <out_dir>

use solder808::{Engine, Slot, Unit};
use std::io::Write;
use std::time::Instant;

const SR: f64 = 48_000.0;

fn render(slot: Slot, unit: Unit, knobs: &[(usize, u32, f64)], seconds: f64) -> Vec<f32> {
    let mut e = Engine::new(SR);
    e.set_unit(unit);
    for &(v, id, val) in knobs {
        e.set_knob(v, id, val);
    }
    e.trigger(slot, 0.5);
    let total = (SR * seconds) as usize;
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        let n = (total - out.len()).min(128);
        e.process(n);
        out.extend_from_slice(&e.out[..n]);
    }
    out
}

fn write_wav(path: &str, s: &[f32]) {
    let mut f = std::fs::File::create(path).expect("create wav");
    let data_len = (s.len() * 2) as u32;
    let mut h = Vec::new();
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&(SR as u32).to_le_bytes());
    h.extend_from_slice(&((SR as u32) * 2).to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    for v in s {
        h.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    f.write_all(&h).expect("write wav");
}

struct Stats {
    peak: f32,
    f0: f64,
    t20: f64,
    centroid: f64,
}

fn stats(s: &[f32]) -> Stats {
    let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // Fundamental from zero crossings in 40..200 ms.
    let a = (SR * 0.04) as usize;
    let b = ((SR * 0.2) as usize).min(s.len());
    let mut zc = 0;
    for i in a + 1..b {
        if (s[i - 1] <= 0.0) != (s[i] <= 0.0) {
            zc += 1;
        }
    }
    let f0 = zc as f64 / 2.0 / ((b - a) as f64 / SR);
    // Time until the envelope stays below -20 dB of peak.
    let thr = peak * 0.1;
    let last = s.iter().rposition(|v| v.abs() > thr).unwrap_or(0);
    let t20 = last as f64 / SR;
    // Spectral centroid of the first 4096 samples (naive DFT on 256 bins).
    let n = 4096.min(s.len());
    let (mut num, mut den) = (0.0, 0.0);
    for k in 1..256 {
        let f = k as f64 * SR / 2.0 / 256.0;
        let w = std::f64::consts::TAU * f / SR;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in s[..n].iter().enumerate() {
            re += *v as f64 * (w * i as f64).cos();
            im -= *v as f64 * (w * i as f64).sin();
        }
        let m = (re * re + im * im).sqrt();
        num += f * m;
        den += m;
    }
    Stats {
        peak,
        f0,
        t20,
        centroid: if den > 0.0 { num / den } else { 0.0 },
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "renders".into());
    std::fs::create_dir_all(&dir).expect("mkdir");
    let new = Unit::default();
    let old = Unit {
        age_years: 45.0,
        ..Default::default()
    };
    let cases: Vec<(&str, Slot, Unit, Vec<(usize, u32, f64)>, f64)> = vec![
        ("bd", Slot::Bd, new, vec![], 1.5),
        ("bd_long", Slot::Bd, new, vec![(0, 3, 1.0)], 2.5),
        ("bd_short", Slot::Bd, new, vec![(0, 3, 0.0)], 1.0),
        ("bd_tune_hi", Slot::Bd, new, vec![(0, 1, 1.0)], 1.5),
        ("bd_tune_lo", Slot::Bd, new, vec![(0, 1, 0.0)], 1.5),
        ("bd_old45y", Slot::Bd, old, vec![], 1.5),
        ("bd_seed1", Slot::Bd, Unit { seed: 1, ..new }, vec![], 1.5),
        ("bd_seed2", Slot::Bd, Unit { seed: 2, ..new }, vec![], 1.5),
        ("sd", Slot::Sd, new, vec![], 1.0),
        ("sd_body", Slot::Sd, new, vec![(1, 2, 0.0)], 1.0),
        ("sd_old45y", Slot::Sd, old, vec![], 1.0),
        ("ch", Slot::Ch, new, vec![], 0.5),
        ("oh", Slot::Oh, new, vec![], 1.5),
        ("cp", Slot::Cp, new, vec![], 1.0),
        ("cp_old45y", Slot::Cp, old, vec![], 1.0),
        ("cb", Slot::Cb, new, vec![], 1.0),
        ("cb_seed1", Slot::Cb, Unit { seed: 1, ..new }, vec![], 1.0),
        ("oh_seed1", Slot::Oh, Unit { seed: 1, ..new }, vec![], 1.5),
    ];
    println!(
        "{:<12} {:>6} {:>8} {:>7} {:>9} {:>7}",
        "case", "peak", "f0(Hz)", "t20(s)", "centroid", "x RT"
    );
    for (name, slot, unit, knobs, secs) in cases {
        let t = Instant::now();
        let s = render(slot, unit, &knobs, secs);
        let rt = secs / t.elapsed().as_secs_f64();
        let st = stats(&s);
        assert!(s.iter().all(|v| v.is_finite()), "{name}: non-finite output");
        println!(
            "{:<12} {:>6.3} {:>8.1} {:>7.3} {:>9.0} {:>7.0}",
            name, st.peak, st.f0, st.t20, st.centroid, rt
        );
        write_wav(&format!("{dir}/{name}.wav"), &s);
    }
    // Shared accent line and supply: BD alone vs BD with every other voice.
    for (label, others) in [("BD alone", false), ("BD + all voices", true)] {
        let mut e = Engine::new(SR);
        // Mute the other voices at the mixer so only BD is measured; they
        // still load the accent line and the supply.
        for v in 1..5 {
            e.set_knob(v, 0, 0.0);
        }
        e.trigger(Slot::Bd, 1.0);
        if others {
            for slot in 1..6 {
                e.trigger(Slot::from_u32(slot).unwrap(), 1.0);
            }
        }
        let mut min_rail = f64::MAX;
        let mut buf = Vec::new();
        for _ in 0..(SR as usize / 128) {
            e.process(128);
            min_rail = min_rail.min(e.rail_volts());
            buf.extend_from_slice(&e.out);
        }
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!("{label:<16} BD peak {peak:.3}, min rail {min_rail:.3} V");
    }
    // Worst case: all six slots on every 16th at 140 BPM.
    for only in 0..6u32 {
        let mut e = Engine::new(SR);
        let step = (SR * 60.0 / 140.0 / 4.0) as usize;
        let total = (SR * 4.0) as usize;
        let t = Instant::now();
        let mut done = 0;
        while done < total {
            if done % step < 128 {
                e.trigger(Slot::from_u32(only).unwrap(), 0.5);
            }
            e.process(128);
            done += 128;
        }
        println!(
            "slot {only} every 16th: x{:.1} realtime",
            4.0 / t.elapsed().as_secs_f64()
        );
    }
    // Worst block right after a single trigger, per slot.
    for slot in 0..6u32 {
        let mut e = Engine::new(SR);
        let mut worst = 0.0f64;
        let mut base = 0.0f64;
        for rep in 0..20 {
            for _ in 0..40 {
                e.process(128);
            }
            e.trigger(Slot::from_u32(slot).unwrap(), 0.35);
            for k in 0..4 {
                let t = Instant::now();
                e.process(128);
                let ms = t.elapsed().as_secs_f64() * 1e3;
                if rep > 0 {
                    if k == 0 {
                        worst = worst.max(ms);
                    } else {
                        base = base.max(ms);
                    }
                }
            }
        }
        println!(
            "slot {slot}: trigger block worst {worst:.3} ms, following blocks worst {base:.3} ms"
        );
    }
    // PPAP bar 1 at 136 BPM: worst single 128-sample block vs its 2.67 ms budget.
    {
        let rows = [
            "x.......x.......",
            "....x.......x...",
            "x.x.x.x.x.x.x.x.",
            "",
            "",
            "....x.......x...",
        ];
        let mut e = Engine::new(SR);
        let step = SR * 60.0 / 136.0 / 4.0;
        let mut next = 0.0;
        let mut idx = 0usize;
        let mut worst = 0.0f64;
        let mut times = Vec::new();
        for b in 0..(SR as usize * 8 / 128) {
            let t0 = (b * 128) as f64;
            while next < t0 + 128.0 {
                for (s, row) in rows.iter().enumerate() {
                    if row.as_bytes().get(idx % 16) == Some(&b'x') {
                        e.trigger(Slot::from_u32(s as u32).unwrap(), 0.35);
                    }
                }
                idx += 1;
                next += step;
            }
            let t = Instant::now();
            e.process(128);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            worst = worst.max(ms);
            if ms > 1.2 {
                println!(
                    "  slow block {b} ({:.0} ms into run): {ms:.3} ms",
                    b as f64 * 128.0 / SR * 1e3
                );
            }
            times.push(ms);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p99 = times[times.len() * 99 / 100];
        println!(
            "PPAP blocks: median {:.3} ms, p99 {:.3} ms, worst {:.3} ms (budget 2.667 ms)",
            times[times.len() / 2],
            p99,
            worst
        );
    }
    let mut e = Engine::new(SR);
    let step = (SR * 60.0 / 140.0 / 4.0) as usize;
    let total = (SR * 8.0) as usize;
    let t = Instant::now();
    let mut done = 0;
    let mut peak = 0.0f32;
    while done < total {
        if done % step < 128 {
            for slot in 0..6 {
                e.trigger(Slot::from_u32(slot).unwrap(), 0.5);
            }
        }
        e.process(128);
        peak = e.out.iter().fold(peak, |m, v| m.max(v.abs()));
        done += 128;
    }
    println!(
        "all slots every 16th: x{:.1} realtime, peak {peak:.3}",
        8.0 / t.elapsed().as_secs_f64()
    );
    // Self-oscillation sweep: decay at max over many unit seeds.
    let mut ringing = 0;
    let mut t20s = Vec::new();
    for seed in 0..40 {
        let s = render(Slot::Bd, Unit { seed, ..new }, &[(0, 3, 1.0)], 4.0);
        let tail = s[s.len() - 4800..]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        if tail > 0.01 {
            ringing += 1;
        }
        t20s.push(stats(&s).t20);
    }
    t20s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "decay=max over 40 seeds: still ringing at 4 s: {ringing}, t20 min/median/max = {:.2}/{:.2}/{:.2}",
        t20s[0], t20s[20], t20s[39]
    );
}
