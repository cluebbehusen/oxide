use super::*;
use std::time::Duration;

#[test]
fn the_preview_worker_draws_a_scenario_off_the_calling_thread() {
    let worker = PreviewWorker::spawn();
    worker.requests.send(None).unwrap();
    let (path, pixels) = worker
        .finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the worker answers");
    assert_eq!(path, None, "the answer names the scenario it drew");
    let pixels = pixels.expect("the embedded skirmish draws");
    assert_eq!(
        pixels.rgba.len(),
        usize::from(pixels.width) * usize::from(pixels.height) * 4
    );
}

#[test]
fn an_unreadable_scenario_answers_with_no_preview() {
    let worker = PreviewWorker::spawn();
    let missing = PathBuf::from("/nonexistent/oxide-preview.json");
    worker.requests.send(Some(missing.clone())).unwrap();
    let (path, pixels) = worker
        .finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the worker answers");
    assert_eq!(path, Some(missing));
    assert!(pixels.is_none());
}
