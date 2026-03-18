// crates/kronk-tui/tests/integration.rs
use kronk_tui::run;

#[tokio::test]
async fn test_tui_starts() {
    // Should fail with "cannot find crate" or "no such module"
    run().await.unwrap();
}