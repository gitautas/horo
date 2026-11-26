//! The Linux camera against real hardware: needs PipeWire and a camera, so
//! it's left out of plain test runs (`cargo test -p horo-ui --test
//! camera_linux -- --ignored`).
#![cfg(target_os = "linux")]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[test]
#[ignore = "needs PipeWire and a camera"]
fn every_camera_delivers_pictures() {
    let cameras = horo_ui::camera::cameras();
    assert!(!cameras.is_empty(), "no cameras found");
    for camera in cameras {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let started = horo_ui::camera::Camera::start_with(
            Some(&camera.id),
            Some(Box::new(move |frame| {
                assert!(frame.width > 0 && frame.height > 0 && frame.width % 2 == 0);
                counter.fetch_add(1, Ordering::Relaxed);
            })),
        );
        let running = started.unwrap_or_else(|e| panic!("{}: {e}", camera.name));
        // A camera takes a moment to settle its exposure; count after that.
        std::thread::sleep(Duration::from_secs(2));
        let settled = count.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(3));
        let latest = running.latest().map(|(_, f)| (f.width, f.height));
        drop(running);
        let pictures = count.load(Ordering::Relaxed) - settled;
        println!("{} ({}): {settled} pictures while settling, then {:.1} a second, last {latest:?}", camera.name, camera.id, pictures as f64 / 3.);
        // How many depends on the camera and the light (webcams slow down
        // to expose longer in the dark); they have to keep coming.
        assert!(pictures >= 3, "{}: only {pictures} pictures in 3 s", camera.name);
    }
}
