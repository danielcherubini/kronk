//! Manual Smoke Tests for Backends Feature
//!
//! This file contains step-by-step instructions for manual testing of the
//! backends install/update UI feature. Run these tests in a real browser
//! environment with the koji-web server running.

/// # Smoke Test #35: Install llama.cpp prebuilt CUDA from a fresh registry
///
/// **Prerequisites:**
/// - koji-web server is running on http://127.0.0.1:8080
/// - Clean registry (no backends installed)
///
/// **Steps:**
/// 1. Open http://127.0.0.1:8080/config in browser
/// 2. Click "Backends" tab
/// 3. Click "Install" button for "llama.cpp"
/// 4. In the modal:
///    - Select "CUDA" for GPU acceleration
///    - Select CUDA version 12.4 (or detected version)
///    - Keep "Build from source" unchecked (should use prebuilt)
///    - Click "Install"
/// 5. Observe:
///    - Modal closes
///    - Job log panel opens with live logs
///    - Logs show download progress
///    - Logs show extraction progress
///    - Job completes with "Succeeded" status
/// 6. Verify:
///    - Backend card shows "Installed: <version>"
///    - Path is set to backends/llama_cpp/...
///    - "Update" button is available

/// # Smoke Test #36: Install ik_llama from source
///
/// **Prerequisites:**
/// - cmake and compiler available on system
///
/// **Steps:**
/// 1. Open http://127.0.0.1:8080/config
/// 2. Click "Backends" tab
/// 3. Click "Install" for "ik_llama.cpp"
/// 4. In the modal:
///    - Note: "ik_llama always builds from source" message appears
///    - "Build from source" is forced on and disabled
///    - Click "Install"
/// 5. Observe:
///    - Job log panel shows:
///      - "Cloning repository..."
///      - "Building from source..."
///      - CMake configure output
///      - Build output
///      - "Backend built and installed at: ..."
///    - Job completes with "Succeeded" status
/// 6. Verify:
///    - Backend card shows installed state
///    - Build directory was cleaned up (no build/ folder)

/// # Smoke Test #37: Trigger update on an installed backend
///
/// **Prerequisites:**
/// - llama.cpp installed
///
/// **Steps:**
/// 1. Open http://127.0.0.1:8080/config
/// 2. Click "Backends" tab
/// 3. Click "Update" button for an installed backend
/// 4. Observe:
///    - Job log panel opens
///    - Logs show update process
///    - Job completes
/// 5. Verify:
///    - Version number changed
///    - Card shows new version

/// # Smoke Test #38: Trigger uninstall
///
/// **Prerequisites:**
/// - A backend installed
///
/// **Steps:**
/// 1. Open http://127.0.0.1:8080/config
/// 2. Click "Backends" tab
/// 3. Click "Delete" button
/// 4. Confirm the dialog
/// 5. Observe:
///    - Backend card disappears or shows "Not installed"
/// 6. Verify:
///    - Backend directory was removed
///    - Registry no longer contains the backend

/// # Smoke Test #39: Reload page mid-install; confirm card rehydrates
///
/// **Prerequisites:**
/// - A long-running install (e.g., source build)
///
/// **Steps:**
/// 1. Start an install that takes > 10 seconds
/// 2. While it's running, reload the page (F5)
/// 3. Observe:
///    - Job log panel automatically reconnects
///    - Logs continue streaming from where they left off
///    - Card shows "Running" state
/// 4. Verify:
///    - SSE stream reconnected successfully
///    - No duplicate log lines

/// # Smoke Test #40: Force a failure (disconnect network)
///
/// **Prerequisites:**
/// - Network control (e.g., disable network adapter)
///
/// **Steps:**
/// 1. Disable network connection
/// 2. Start an install that requires download
/// 3. Observe:
///    - Job log shows error
///    - Job status changes to "Failed"
///    - Retry button appears
/// 4. Re-enable network
/// 5. Click "Retry"
/// 6. Observe:
///    - Modal reopens with previous settings
///    - Install completes successfully

/// # Smoke Test #41: Concurrent job rejection (409)
///
/// **Steps:**
/// 1. Start an install
/// 2. While it's running, try to start another install
/// 3. Observe:
///    - Second install fails with 409 Conflict
///    - Error message: "another backend job is already running"
/// 4. Verify:
///    - First job continues uninterrupted

/// # Smoke Test #42: Loopback banner appears when bound to non-loopback
///
/// **Steps:**
/// 1. Start koji-web with `KOJI_BOUND_TO_LOOPBACK=false`
/// 2. Open http://127.0.0.1:8080/config
/// 3. Observe:
///    - Warning banner appears at top of Backends section
///    - Message: "⚠ Warning: The web UI is bound to a non-loopback address..."
/// 4. Verify:
///    - Banner does not appear when `KOJI_BOUND_TO_LOOPBACK=true`

/// # Smoke Test #43: Same-origin enforcement
///
/// **Steps:**
/// 1. Open browser dev tools (Network tab)
/// 2. Try to POST to /api/backends/install with Origin: http://evil.example
/// 3. Observe:
///    - Request is rejected with 403 Forbidden
/// 4. Verify:
///    - Same-origin requests (Origin: http://127.0.0.1:8080) succeed

/// # Smoke Test #44: Advanced disclosure for custom backends
///
/// **Steps:**
/// 1. Add a custom backend to config (backend_type: "custom")
/// 2. Open http://127.0.0.1:8080/config
/// 3. Click "Backends" tab
/// 4. For custom backend card, click "Advanced" disclosure
/// 5. Observe:
///    - Path, default_args, health_check_url fields are editable
///    - For installed backends, path is read-only with "Reset" button
/// 6. Verify:
///    - Changes persist after save

fn _smoke_tests_module_marker() {}
