pub mod linux;
// Windows and BSD support planned for future releases

#[cfg(not(target_os = "linux"))]
compile_error!("Tama currently only supports Linux. BSD support is planned.");
