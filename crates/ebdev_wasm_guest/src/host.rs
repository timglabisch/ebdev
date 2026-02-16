//! FFI Layer für Host-Kommunikation

extern "C" {
    /// Host-Function: sendet JSON Request, empfängt JSON Response
    /// Return: (response_ptr << 32) | response_len
    #[link_name = "call"]
    fn ebdev_call(request_ptr: *const u8, request_len: u32) -> i64;
}

/// Exportierte Alloc-Funktion die der Host aufruft um Speicher im Guest zu allozieren
#[no_mangle]
pub extern "C" fn ebdev_alloc(size: i32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { std::alloc::alloc(layout) }
}

/// Sendet einen JSON Request an den Host und gibt die Response zurück
pub fn call_host(request_json: &[u8]) -> Option<Vec<u8>> {
    let result = unsafe { ebdev_call(request_json.as_ptr(), request_json.len() as u32) };

    if result == 0 {
        return None;
    }

    let response_ptr = (result >> 32) as usize;
    let response_len = (result & 0xFFFFFFFF) as usize;

    if response_len == 0 {
        return None;
    }

    let response = unsafe {
        std::slice::from_raw_parts(response_ptr as *const u8, response_len).to_vec()
    };

    Some(response)
}
