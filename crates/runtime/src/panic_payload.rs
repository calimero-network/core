use std::any::Any;

/// Extracts a human-readable message from a panic payload.
/// Panics can carry either a `&'static str` or a `String` as their message.
pub(crate) fn panic_payload_to_string(payload: &(dyn Any + Send), fallback: &str) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::panic_payload_to_string;
    use std::any::Any;

    #[test]
    fn test_panic_payload_to_string_with_str() {
        let payload: Box<dyn Any + Send> = Box::new("test panic message");
        let message = panic_payload_to_string(payload.as_ref(), "<unknown panic>");
        assert_eq!(message, "test panic message");
    }

    #[test]
    fn test_panic_payload_to_string_with_string() {
        let payload: Box<dyn Any + Send> = Box::new(String::from("owned panic message"));
        let message = panic_payload_to_string(payload.as_ref(), "<unknown panic>");
        assert_eq!(message, "owned panic message");
    }

    #[test]
    fn test_panic_payload_to_string_with_unknown_type() {
        let payload: Box<dyn Any + Send> = Box::new(42_i32);
        let message = panic_payload_to_string(payload.as_ref(), "<unknown panic>");
        assert_eq!(message, "<unknown panic>");
    }
}
