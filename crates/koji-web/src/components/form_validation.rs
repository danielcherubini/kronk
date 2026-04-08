//! Form validation helper functions.
//!
//! Pure logic functions with unit tests for validating form inputs.

use std::net::IpAddr;
use url::Url;

/// Validate an IP address (IPv4 or IPv6).
///
/// Uses `std::net::IpAddr::from_str` which handles both IPv4 and IPv6.
#[allow(dead_code)]
pub fn validate_ip_address(s: &str) -> Result<(), String> {
    s.parse::<IpAddr>()
        .map(|_| ())
        .map_err(|e| format!("Invalid IP address: {e}"))
}

/// Validate a port number (1-65535).
#[allow(dead_code)]
pub fn validate_port(port: u16) -> Result<(), String> {
    if port == 0 {
        Err("Port must be between 1 and 65535".to_string())
    } else {
        Ok(())
    }
}

/// Validate a URL.
///
/// Uses `url::Url::parse` for robust validation.
#[allow(dead_code)]
pub fn validate_url(s: &str) -> Result<(), String> {
    Url::parse(s)
        .map(|_| ())
        .map_err(|e| format!("Invalid URL: {e}"))
}

/// Validate a range for f64 values.
#[allow(dead_code)]
pub fn validate_range_f64(value: f64, min: f64, max: f64) -> Result<(), String> {
    if value < min || value > max {
        Err(format!("Value must be between {min} and {max}"))
    } else {
        Ok(())
    }
}

/// Validate a range for u16 values.
#[allow(dead_code)]
pub fn validate_range_u16(value: u16, min: u16, max: u16) -> Result<(), String> {
    if value < min || value > max {
        Err(format!("Value must be between {min} and {max}"))
    } else {
        Ok(())
    }
}

/// Validate a range for u32 values.
#[allow(dead_code)]
pub fn validate_range_u32(value: u32, min: u32, max: u32) -> Result<(), String> {
    if value < min || value > max {
        Err(format!("Value must be between {min} and {max}"))
    } else {
        Ok(())
    }
}

/// Validate that a string is not empty.
#[allow(dead_code)]
pub fn validate_required(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        Err("This field is required".to_string())
    } else {
        Ok(())
    }
}

/// Get the checked state from a checkbox event.
#[cfg(feature = "csr")]
#[allow(clippy::extra_unused_lifetimes, clippy::borrowed_box, dead_code)]
pub fn event_target_checked(e: &web_sys::Event) -> bool {
    use wasm_bindgen::JsCast;
    let target = e.target();
    match target {
        Some(t) => t
            .dyn_ref::<web_sys::HtmlInputElement>()
            .map(|input| input.checked())
            .unwrap_or(false),
        None => false,
    }
}

/// Get the value from an input event.
#[cfg(feature = "csr")]
#[allow(clippy::extra_unused_lifetimes, clippy::borrowed_box, dead_code)]
pub fn event_target_value(e: &web_sys::Event) -> String {
    use wasm_bindgen::JsCast;
    let target = e.target();
    match target {
        Some(t) => t
            .dyn_ref::<web_sys::HtmlInputElement>()
            .map(|input| input.value())
            .unwrap_or_default(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ip_address_valid_ipv4() {
        assert!(validate_ip_address("192.168.1.1").is_ok());
        assert!(validate_ip_address("127.0.0.1").is_ok());
        assert!(validate_ip_address("0.0.0.0").is_ok());
    }

    #[test]
    fn test_validate_ip_address_valid_ipv6() {
        assert!(validate_ip_address("::1").is_ok());
        assert!(validate_ip_address("2001:0db8:85a3:0000:0000:8a2e:0370:7334").is_ok());
        assert!(validate_ip_address("2001:db8::1").is_ok());
    }

    #[test]
    fn test_validate_ip_address_invalid() {
        assert!(validate_ip_address("256.256.256.256").is_err());
        assert!(validate_ip_address("not-an-ip").is_err());
        assert!(validate_ip_address("").is_err());
    }

    #[test]
    fn test_validate_port() {
        assert!(validate_port(1).is_ok());
        assert!(validate_port(8080).is_ok());
        assert!(validate_port(65535).is_ok());
        assert!(validate_port(0).is_err());
    }

    #[test]
    fn test_validate_url_valid() {
        assert!(validate_url("http://localhost:11434").is_ok());
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://192.168.1.1:8080/api").is_ok());
    }

    #[test]
    fn test_validate_url_invalid() {
        assert!(validate_url("not-a-url").is_err());
        assert!(validate_url("http://").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn test_validate_range_f64() {
        assert!(validate_range_f64(0.5, 0.0, 1.0).is_ok());
        assert!(validate_range_f64(0.0, 0.0, 1.0).is_ok());
        assert!(validate_range_f64(1.0, 0.0, 1.0).is_ok());
        assert!(validate_range_f64(-0.1, 0.0, 1.0).is_err());
        assert!(validate_range_f64(1.1, 0.0, 1.0).is_err());
    }

    #[test]
    fn test_validate_range_u16() {
        assert!(validate_range_u16(100, 1, 65535).is_ok());
        assert!(validate_range_u16(1, 1, 65535).is_ok());
        assert!(validate_range_u16(65535, 1, 65535).is_ok());
        assert!(validate_range_u16(0, 1, 65535).is_err());
        // Can't test 65536 as it overflows u16
    }

    #[test]
    fn test_validate_required() {
        assert!(validate_required("hello").is_ok());
        assert!(validate_required(" ").is_err());
        assert!(validate_required("").is_err());
    }
}
