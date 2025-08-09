#![allow(clippy::missing_safety_doc)]

// Minimal plain-ABI exports for use inside AudioWorklet
// No wasm-bindgen to keep surface lean; use linear memory directly.

static mut INPUT_PTR: *mut f32 = core::ptr::null_mut();
static mut INPUT_CAPACITY: usize = 0;
static mut WRITE_POS: usize = 0;

#[no_mangle]
pub unsafe extern "C" fn init(capacity: usize) -> *mut f32 {
    // Allocate a zeroed input ring buffer owned by Rust and return its pointer
    let mut buf: Vec<f32> = vec![0.0; capacity];
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf); // leak ownership; managed manually for WASM lifetime
    INPUT_PTR = ptr;
    INPUT_CAPACITY = capacity;
    WRITE_POS = 0;
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn get_input_ptr() -> *mut f32 { INPUT_PTR }

#[no_mangle]
pub unsafe extern "C" fn get_input_capacity() -> usize { INPUT_CAPACITY }

#[no_mangle]
pub unsafe extern "C" fn get_write_pos() -> usize { WRITE_POS }

#[no_mangle]
pub unsafe extern "C" fn set_write_pos(pos: usize) { WRITE_POS = pos % INPUT_CAPACITY; }

#[no_mangle]
pub unsafe extern "C" fn process_quantum(_n: usize) {
    // Placeholder for future DSP. Intentionally empty.
}


