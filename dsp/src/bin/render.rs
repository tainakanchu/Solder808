//! Native check: render single hits to WAV and print simple measurements.
//! Usage: cargo run --release --bin render -- <out_dir>
//!
//! Panics if a tom or conga lands outside its intended pitch range, so it
//! doubles as a listening proxy after circuit changes.

use solder808::{Engine, Slot, Unit, SLOTS};
use std::io::Write;
use std::time::Instant;

const SR: f64 = 48_000.0;

/// One rendered hit: knobs are (voice, id, value), switches (voice, value).
struct Case {
    name: String,
    slot: Slot,
    unit: Unit,
    knobs: Vec<(usize, u32, f64)>,
    switches: Vec<(usize, u32)>,
    secs: f64,
}

fn case(name: &str, slot: Slot, unit: Unit, knobs: &[(usize, u32, f64)], secs: f64) -> Case {
    Case {
        name: name.into(),
        slot,
        unit,
        knobs: knobs.to_vec(),
        switches: Vec::new(),
        secs,
    }
}

fn switched(mut c: Case, voice: usize, value: u32) -> Case {
    c.switches.push((voice, value));
    c
}

fn render(c: &Case) -> Vec<f32> {
    let mut e = Engine::new(SR);
    e.set_unit(c.unit);
    for &(v, id, val) in &c.knobs {
        e.set_knob(v, id, val);
    }
    for &(v, val) in &c.switches {
        e.set_switch(v, val);
    }
    e.trigger(c.slot, 0.5);
    let total = (SR * c.secs) as usize;
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
    rms: f64,
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
    // RMS over the sounding part (to -20 dB).
    let n = last.max(480).min(s.len());
    let rms = (s[..n].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
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
        rms,
        f0,
        t20,
        centroid: if den > 0.0 { num / den } else { 0.0 },
    }
}

/// Strongest frequency in `lo..hi` Hz over `t0..t1` s (Hann window, 0.3 %
/// log-spaced DFT scan).
fn dft_peak(s: &[f32], t0: f64, t1: f64, lo: f64, hi: f64) -> f64 {
    let w = &s[(t0 * SR) as usize..((t1 * SR) as usize).min(s.len())];
    let n = w.len() as f64;
    let (mut best, mut at, mut f) = (0.0, 0.0, lo);
    while f < hi {
        let dw = std::f64::consts::TAU * f / SR;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in w.iter().enumerate() {
            let h = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
            re += *v as f64 * h * (dw * i as f64).cos();
            im += *v as f64 * h * (dw * i as f64).sin();
        }
        if re * re + im * im > best {
            best = re * re + im * im;
            at = f;
        }
        f *= 1.003;
    }
    at
}

/// Mean frequency from upward zero crossings over `t0..t1` s.
fn zc_freq(s: &[f32], t0: f64, t1: f64) -> f64 {
    let (a, b) = ((t0 * SR) as usize, ((t1 * SR) as usize).min(s.len()));
    let mut ups = Vec::new();
    for i in a.max(1)..b {
        if s[i - 1] <= 0.0 && s[i] > 0.0 {
            ups.push((i - 1) as f64 + (-s[i - 1] / (s[i] - s[i - 1])) as f64);
        }
    }
    if ups.len() < 2 {
        return 0.0;
    }
    (ups.len() - 1) as f64 * SR / (ups[ups.len() - 1] - ups[0])
}

/// Seconds of audio per second of CPU for 16-step rows (one per slot).
fn pattern_speed(rows: &[&str], bpm: f64, secs: f64) -> (f64, f32) {
    let mut e = Engine::new(SR);
    let step = SR * 60.0 / bpm / 4.0;
    let (mut next, mut idx, mut peak) = (0.0, 0usize, 0.0f32);
    let t = Instant::now();
    for b in 0..(SR * secs) as usize / 128 {
        let t0 = (b * 128) as f64;
        while next < t0 + 128.0 {
            for (s, row) in rows.iter().enumerate() {
                if row.as_bytes().get(idx % 16) == Some(&b'x') {
                    e.trigger(Slot::from_u32(s as u32).unwrap(), 0.5);
                }
            }
            idx += 1;
            next += step;
        }
        e.process(128);
        peak = e.out.iter().fold(peak, |m, v| m.max(v.abs()));
    }
    (secs / t.elapsed().as_secs_f64(), peak)
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "renders".into());
    std::fs::create_dir_all(&dir).expect("mkdir");
    let new = Unit::default();
    let old = Unit {
        age_years: 45.0,
        ..Default::default()
    };
    let s1 = Unit { seed: 1, ..new };
    let s2 = Unit { seed: 2, ..new };
    let mut cases = vec![
        case("bd", Slot::Bd, new, &[], 1.5),
        case("bd_long", Slot::Bd, new, &[(0, 3, 1.0)], 2.5),
        case("bd_short", Slot::Bd, new, &[(0, 3, 0.0)], 1.0),
        case("bd_tune_hi", Slot::Bd, new, &[(0, 1, 1.0)], 1.5),
        case("bd_tune_lo", Slot::Bd, new, &[(0, 1, 0.0)], 1.5),
        case("bd_old45y", Slot::Bd, old, &[], 1.5),
        case("bd_seed1", Slot::Bd, s1, &[], 1.5),
        case("bd_seed2", Slot::Bd, s2, &[], 1.5),
        case("sd", Slot::Sd, new, &[], 1.0),
        case("sd_body", Slot::Sd, new, &[(1, 2, 0.0)], 1.0),
        case("sd_old45y", Slot::Sd, old, &[], 1.0),
        case("ch", Slot::Ch, new, &[], 0.5),
        case("oh", Slot::Oh, new, &[], 1.5),
        case("oh_seed1", Slot::Oh, s1, &[], 1.5),
        case("cp", Slot::Cp, new, &[], 1.0),
        case("cp_old45y", Slot::Cp, old, &[], 1.0),
        switched(case("ma", Slot::Cp, new, &[], 0.5), 3, 1),
        switched(case("ma_old45y", Slot::Cp, old, &[], 0.5), 3, 1),
        case("cb", Slot::Cb, new, &[], 1.0),
        case("cb_seed1", Slot::Cb, s1, &[], 1.0),
        case("rs", Slot::Rs, new, &[], 0.5),
        case("rs_old45y", Slot::Rs, old, &[], 0.5),
        switched(case("cl", Slot::Rs, new, &[], 0.5), 8, 1),
        switched(case("cl_seed1", Slot::Rs, s1, &[], 0.5), 8, 1),
        case("cy", Slot::Cy, new, &[], 3.0),
        case("cy_tone0", Slot::Cy, new, &[(9, 1, 0.0)], 3.0),
        case("cy_tone1", Slot::Cy, new, &[(9, 1, 1.0)], 3.0),
        case("cy_decay0", Slot::Cy, new, &[(9, 2, 0.0)], 1.0),
        case("cy_decay1", Slot::Cy, new, &[(9, 2, 1.0)], 4.0),
        case("cy_old45y", Slot::Cy, old, &[], 3.0),
    ];
    // Toms and congas: every channel, mode and tuning corner, plus aged units.
    let chans = [
        ("lt", "lc", Slot::Lt, 5),
        ("mt", "mc", Slot::Mt, 6),
        ("ht", "hc", Slot::Ht, 7),
    ];
    for &(tom, conga, slot, v) in &chans {
        for (k, x) in [("0", 0.0), ("5", 0.5), ("1", 1.0)] {
            cases.push(case(
                &format!("{tom}_tune{k}"),
                slot,
                new,
                &[(v, 1, x)],
                1.0,
            ));
            let c = case(&format!("{conga}_tune{k}"), slot, new, &[(v, 2, x)], 1.0);
            cases.push(switched(c, v, 1));
        }
        cases.push(case(&format!("{tom}_old45y"), slot, old, &[], 1.0));
        cases.push(switched(
            case(&format!("{conga}_old45y"), slot, old, &[], 1.0),
            v,
            1,
        ));
    }
    println!(
        "{:<13} {:>6} {:>6} {:>8} {:>7} {:>9} {:>7}",
        "case", "peak", "rms", "f0(Hz)", "t20(s)", "centroid", "x RT"
    );
    let mut toms = Vec::new();
    for c in &cases {
        let t = Instant::now();
        let s = render(c);
        let rt = c.secs / t.elapsed().as_secs_f64();
        let st = stats(&s);
        assert!(
            s.iter().all(|v| v.is_finite()),
            "{}: non-finite output",
            c.name
        );
        println!(
            "{:<13} {:>6.3} {:>6.3} {:>8.1} {:>7.3} {:>9.0} {:>7.0}",
            c.name, st.peak, st.rms, st.f0, st.t20, st.centroid, rt
        );
        write_wav(&format!("{dir}/{}.wav", c.name), &s);
        let tom_or_conga = ["lt", "mt", "ht", "lc", "mc", "hc"]
            .iter()
            .any(|p| c.name.starts_with(p));
        if tom_or_conga && c.name.contains("_tune") {
            toms.push((c.name.clone(), s));
        }
    }

    // Tom / conga pitch: the strongest partial over 20–300 ms (includes the
    // downward bend), the settled pitch from zero crossings over 80–160 ms,
    // and the bend from the first 30 ms.
    println!("\npitch          dft20-300ms  settled  start(3-30ms)  [intended settled range]");
    let ranges = [
        ("lt", 75.0, 115.0),
        ("mt", 110.0, 170.0),
        ("ht", 155.0, 225.0),
        ("lc", 160.0, 235.0),
        ("mc", 225.0, 330.0),
        ("hc", 315.0, 460.0),
    ];
    for (name, s) in &toms {
        let f = dft_peak(s, 0.02, 0.3, 40.0, 1000.0);
        let settled = zc_freq(s, 0.08, 0.16);
        let start = zc_freq(s, 0.003, 0.03);
        let &(_, lo, hi) = ranges.iter().find(|r| name.starts_with(r.0)).unwrap();
        println!("{name:<13} {f:>10.1}  {settled:>7.1}  {start:>13.1}  [{lo:.0}, {hi:.0}]");
        assert!(
            (lo..=hi).contains(&settled) && (lo..=hi * 1.15).contains(&f),
            "{name}: pitch {settled:.1} Hz (dft {f:.1}) outside {lo}..{hi}"
        );
        assert!(start > settled, "{name}: no downward bend");
    }

    // Shared accent line and supply: BD alone vs BD with every other channel.
    println!();
    for (label, others) in [("BD alone", false), ("BD + all channels", true)] {
        let mut e = Engine::new(SR);
        // Mute the other voices at the mixer so only BD is measured; they
        // still load the accent line and the supply.
        for v in 1..solder808::VOICES {
            e.set_knob(v, 0, 0.0);
        }
        e.trigger(Slot::Bd, 1.0);
        if others {
            for slot in 1..SLOTS {
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
        println!("{label:<18} BD peak {peak:.3}, min rail {min_rail:.3} V");
    }

    // Load: each slot alone on every 16th at 120 BPM.
    println!();
    for only in 0..SLOTS {
        let mut rows = vec![""; SLOTS as usize];
        rows[only as usize] = "xxxxxxxxxxxxxxxx";
        let (x, _) = pattern_speed(&rows, 120.0, 4.0);
        println!("slot {only:>2} every 16th: x{x:.1} realtime");
    }
    // Worst block right after a single trigger, per slot.
    for slot in 0..SLOTS {
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
            "slot {slot:>2}: trigger block worst {worst:.3} ms, following blocks worst {base:.3} ms"
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
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "PPAP blocks: median {:.3} ms, p99 {:.3} ms, worst {:.3} ms (budget 2.667 ms)",
            times[times.len() / 2],
            times[times.len() * 99 / 100],
            times[times.len() - 1]
        );
    }
    // Whole-machine load at 120 BPM: (a) every channel on every 16th, the
    // worst case; (b) a typical pattern touching every channel.
    let all = vec!["xxxxxxxxxxxxxxxx"; SLOTS as usize];
    let typical = [
        "x...x...x...x...",
        "....x.......x...",
        "x.x.x.x.x.x.x.x.",
        "..............x.",
        "............x...",
        "................",
        "..........x.....",
        "...........x....",
        ".............x..",
        "..x.....x..x....",
        "x...............",
    ];
    let best = |rows: &[&str]| {
        (0..3)
            .map(|_| pattern_speed(rows, 120.0, 8.0))
            .fold((0.0f64, 0.0f32), |m, r| (m.0.max(r.0), r.1))
    };
    let (x, peak) = best(&all);
    println!("all {SLOTS} slots every 16th @120: x{x:.2} realtime, peak {peak:.3}");
    let (x, peak) = best(&typical);
    println!("typical 11-channel pattern @120: x{x:.2} realtime, peak {peak:.3}");
    // Self-oscillation sweep: decay at max over many unit seeds.
    let mut ringing = 0;
    let mut t20s = Vec::new();
    for seed in 0..40 {
        let s = render(&case(
            "",
            Slot::Bd,
            Unit { seed, ..new },
            &[(0, 3, 1.0)],
            4.0,
        ));
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
