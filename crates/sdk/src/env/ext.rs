use super::{expected_boolean, expected_register, panic_str, read_register, DATA_REGISTER};
use crate::sys;

#[doc(hidden)]
pub unsafe fn fetch(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let headers = match borsh::to_vec(&headers) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize headers: {:?}", err)),
    };
    let method = sys::Buffer::from(method);
    let url = sys::Buffer::from(url);
    let headers = sys::Buffer::from(headers.as_slice());
    let body = sys::Buffer::from(body);

    let failed = unsafe { sys::fetch(url, method, headers, body, DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean);
    let data = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
    if failed {
        return Err(String::from_utf8(data).unwrap_or_else(|_| {
            panic_str("Fetch failed with an error but the error is an invalid UTF-8 string.")
        }));
    }

    Ok(data)
}
