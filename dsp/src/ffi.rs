//! Plain C ABI for the AudioWorklet (no wasm-bindgen: its glue code expects
//! APIs such as TextDecoder that AudioWorkletGlobalScope may not provide).

use crate::engine::{Engine, Slot, BLOCK};
use crate::parts::Unit;

/// # Safety
/// Returns an owned pointer; free it with `engine_free`.
#[no_mangle]
pub extern "C" fn engine_new(sample_rate: f64) -> *mut Engine {
    Box::into_raw(Box::new(Engine::new(sample_rate)))
}

/// # Safety
/// `e` must come from `engine_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn engine_free(e: *mut Engine) {
    if !e.is_null() {
        drop(Box::from_raw(e));
    }
}

macro_rules! eng {
    ($e:expr) => {
        // SAFETY: the JS side only passes the pointer returned by engine_new.
        unsafe { &mut *$e }
    };
}

#[no_mangle]
pub extern "C" fn engine_block_size() -> u32 {
    BLOCK as u32
}

#[no_mangle]
pub extern "C" fn engine_out_ptr(e: *mut Engine) -> *const f32 {
    eng!(e).out.as_ptr()
}

#[no_mangle]
pub extern "C" fn engine_process(e: *mut Engine, frames: u32) {
    eng!(e).process(frames as usize);
}

#[no_mangle]
pub extern "C" fn engine_trigger(e: *mut Engine, slot: u32, accent: f64) {
    if let Some(s) = Slot::from_u32(slot) {
        eng!(e).trigger(s, accent);
    }
}

#[no_mangle]
pub extern "C" fn engine_set_knob(e: *mut Engine, voice: u32, id: u32, value: f64) {
    eng!(e).set_knob(voice as usize, id, value);
}

#[no_mangle]
pub extern "C" fn engine_set_master(e: *mut Engine, value: f64) {
    eng!(e).master = value.clamp(0.0, 2.0);
}

#[no_mangle]
pub extern "C" fn engine_set_accent(e: *mut Engine, value: f64) {
    eng!(e).set_accent(value);
}

#[no_mangle]
pub extern "C" fn engine_set_unit(e: *mut Engine, seed: u32, age_years: f64, temp_c: f64) {
    eng!(e).set_unit(Unit {
        seed,
        age_years,
        temp_c,
    });
}

#[no_mangle]
pub extern "C" fn engine_set_part_scale(e: *mut Engine, voice: u32, part: u32, scale: f64) {
    eng!(e).set_part_scale(voice as usize, part as usize, scale);
}

#[no_mangle]
pub extern "C" fn engine_part_count(e: *mut Engine, voice: u32) -> u32 {
    eng!(e).parts(voice as usize).map_or(0, |p| p.len() as u32)
}

fn part(e: *mut Engine, voice: u32, i: u32) -> Option<&'static crate::parts::Part> {
    let eng: &'static Engine = eng!(e);
    eng.parts(voice as usize)?.get(i as usize)
}

#[no_mangle]
pub extern "C" fn engine_part_name_ptr(e: *mut Engine, voice: u32, i: u32) -> *const u8 {
    part(e, voice, i).map_or(std::ptr::null(), |p| p.name.as_ptr())
}

#[no_mangle]
pub extern "C" fn engine_part_name_len(e: *mut Engine, voice: u32, i: u32) -> u32 {
    part(e, voice, i).map_or(0, |p| p.name.len() as u32)
}

#[no_mangle]
pub extern "C" fn engine_part_kind(e: *mut Engine, voice: u32, i: u32) -> u32 {
    part(e, voice, i).map_or(0, |p| p.kind as u32)
}

#[no_mangle]
pub extern "C" fn engine_part_nominal(e: *mut Engine, voice: u32, i: u32) -> f64 {
    part(e, voice, i).map_or(0.0, |p| p.nominal)
}

#[no_mangle]
pub extern "C" fn engine_part_effective(e: *mut Engine, voice: u32, i: u32) -> f64 {
    part(e, voice, i).map_or(0.0, |p| p.effective)
}

#[no_mangle]
pub extern "C" fn engine_part_scale(e: *mut Engine, voice: u32, i: u32) -> f64 {
    part(e, voice, i).map_or(1.0, |p| p.user_scale)
}
