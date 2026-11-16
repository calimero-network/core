//! Python <-> JSON, and path segments that cannot reach another route.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::Value;

/// Python object -> JSON, through Python's own `json` module.
pub(crate) fn to_json(py: Python<'_>, obj: Option<&Bound<'_, PyAny>>) -> PyResult<Value> {
    let Some(obj) = obj else {
        return Ok(Value::Object(serde_json::Map::new()));
    };
    if obj.is_none() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let s: String = py
        .import_bound("json")?
        .call_method1("dumps", (obj,))?
        .extract()?;
    serde_json::from_str(&s).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// JSON -> Python object.
pub(crate) fn to_py(py: Python<'_>, v: &impl serde::Serialize) -> PyResult<PyObject> {
    let s = serde_json::to_string(v).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(py
        .import_bound("json")?
        .call_method1("loads", (s,))?
        .unbind())
}

/// One path segment, percent-encoded, so an id can never reach another route.
pub(crate) fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::seg;

    #[test]
    fn a_path_segment_cannot_reach_another_route() {
        assert_eq!(seg("39VM3IF4orLKKYdSYpZgqg"), "39VM3IF4orLKKYdSYpZgqg");
        assert_eq!(seg("a/../ops/status"), "a%2F..%2Fops%2Fstatus");
        assert_eq!(seg("x?y=1#z"), "x%3Fy%3D1%23z");
        assert_eq!(seg("a b"), "a%20b");
    }
}
