use llmhelper::source::{cache_handle, OmpSource, Source};

#[test]
fn warm_message_cache_never_removes_omp_records() {
    let cache_dir = tempfile::tempdir().unwrap();
    let sessions = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/omp");
    let expected = OmpSource::new(sessions.clone()).load().unwrap();
    assert!(!expected.is_empty(), "fixture must contain usage records");
    let source = OmpSource::with_cache(sessions.clone(), cache_handle(cache_dir.path(), "omp"));
    assert!(!source.load_messages().unwrap().is_empty());
    source.flush_cache();
    assert_eq!(
        source.load().unwrap(),
        expected,
        "warm handle must preserve records"
    );
    let reopened = OmpSource::with_cache(sessions, cache_handle(cache_dir.path(), "omp"));
    assert_eq!(
        reopened.load().unwrap(),
        expected,
        "persisted cache must preserve records"
    );
}
