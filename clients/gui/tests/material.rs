//! The host against a **real** segment: one the server wrote, and the material
//! beside it.
//!
//! The layout itself is `clausters_core::shm` now, tested there — so what is
//! left to check here is the part that is genuinely this crate's: that a host
//! maps what a server published, reads the planes it draws from, and edits the
//! material in place. Gated on `standalone`, the feature that links the server,
//! because that is what makes a real segment available to build one from.
//!
//! This replaced three tests that built a segment file by hand from this
//! crate's own idea of the layout — a mirror tested against itself, which is
//! exactly the shape of check that passed for a week while the reader refused
//! every valid segment.

#![cfg(all(feature = "standalone", unix))]

use std::sync::Arc;

use clausters::dsp::region::Region;
use clausters::server::ipc::Segment;
use clausters_gui::host::material::SharedMaterial;
use clausters_gui::host::shm::SharedSegment;

/// A segment path nothing else in this run uses.
fn scratch(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("clausters-gui-{tag}-{}", std::process::id()))
}

#[test]
fn the_host_reads_the_segment_the_server_wrote() {
    let path = scratch("mirror");
    let _ = std::fs::remove_file(&path);
    let server = Segment::create(&path).expect("segment");
    server.set_sample_rate(48_000.0);

    let host = SharedSegment::open(&path).expect("the host maps what the server created");
    assert_eq!(host.control_buses(), server.control_bus_count());
    assert_eq!(host.sample_rate(), 48_000.0);
    assert!(
        host.buffer_rows() > 0,
        "a v9 segment carries a buffer directory, and the mirror has to see it"
    );

    let _ = std::fs::remove_file(&path);
}

/// What the host actually reads each frame, against a segment the **server**
/// wrote: the buses, the levels, the tap windows and the clocks.
///
/// This used to be three tests over a segment file this crate built by hand,
/// which is a test of one mirror against itself. Reading the real thing is the
/// only version of it worth having.
#[test]
fn the_buses_the_levels_and_the_taps_read_what_the_server_published() {
    let path = scratch("planes");
    let _ = std::fs::remove_file(&path);
    let server = Segment::create_full(&path, 8, 2, 256).expect("segment");
    server.set_sample_rate(44_100.0);
    server.view().set_control(2, -0.75);
    server.set_level(1, 0.5);
    server.set_tap_of_bus(1, Some(0));
    server
        .clock()
        .store(4096, std::sync::atomic::Ordering::Release);
    server
        .transport_position()
        .store(1024, std::sync::atomic::Ordering::Release);
    let block = [0.25f32; clausters::server::engine::BLOCK_SIZE];
    server.tap_write(0, &block);

    let host = SharedSegment::open(&path).expect("the host maps it");
    assert_eq!(host.control_buses(), 8);
    assert_eq!(host.control(2), -0.75);
    assert_eq!(host.control(99), 0.0, "an out-of-range bus reads silence");
    assert_eq!(host.sample_rate(), 44_100.0);
    assert_eq!(host.taps(), 2);
    assert_eq!(host.tap_frames(), 256);
    assert_eq!(host.level(1), 0.5);
    assert_eq!(host.tap_of_bus(1), Some(0));
    assert_eq!(host.tap_of_bus(0), None);
    assert_eq!(host.sample_clock(), 4096);
    assert_eq!(host.transport_position(), 1024);

    let mut out = [0.0f32; 32];
    assert_eq!(
        host.tap_read_latest(0, &mut out),
        Some(block.len() as u64),
        "the window the engine just wrote"
    );
    assert!(out.iter().all(|&s| s == 0.25));
    let mut too_big = [0.0f32; 129];
    assert_eq!(
        host.tap_read_latest(0, &mut too_big),
        None,
        "past half the ring, a window cannot be read without racing the writer"
    );
    assert_eq!(host.tap_read_latest(9, &mut out), None, "no such tap");

    let _ = std::fs::remove_file(&path);
}

/// A file that is not a segment of this version is refused, not read.
#[test]
fn a_foreign_or_stale_segment_is_refused() {
    let path = scratch("refuse");
    std::fs::write(&path, b"short").expect("write");
    assert!(SharedSegment::open(&path).is_err(), "not a segment at all");

    Segment::create_full(&path, 8, 0, 256).expect("segment");
    assert!(SharedSegment::open(&path).is_ok());
    // Corrupt the version field (the header's second word).
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[4..8].copy_from_slice(&999u32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");
    let err = match SharedSegment::open(&path) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a version mismatch must refuse to attach"),
    };
    assert!(err.contains("ABI version"), "{err}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_take_the_server_published_is_drawn_and_edited_in_place() {
    let path = scratch("material");
    let _ = std::fs::remove_file(&path);
    let server = Segment::create(&path).expect("segment");

    // The server publishes a take: a directory row and a region beside the
    // segment, which is what `share_buffer` does for every buffer it installs.
    let generation = server.publish_buffer(4, 8, 2, 44_100.0).expect("a row");
    let region_path = Region::path_for(&path, 4, generation);
    let region = Region::create(&region_path, 16).expect("region");
    region.cells()[2 * 2].store(0.5f32.to_bits(), std::sync::atomic::Ordering::Relaxed);

    let material = SharedMaterial::new(
        Arc::new(SharedSegment::open(&path).expect("segment")),
        path.clone(),
    );
    assert!(material.holds(4));
    let take = material.map(4).expect("the host maps the take");
    assert_eq!(take.shape(), (2, 8, 44_100.0));
    assert_eq!(
        take.read_all()[4],
        0.5,
        "drawing a take reads the server's own memory"
    );

    // And the edit goes the other way, with nothing sent: the cells the hand
    // moved are the cells the engine plays next block.
    take.write_channel(1, 3, &[-0.25, -0.5]);
    let cells = region.cells();
    let at = |cell: usize| f32::from_bits(cells[cell].load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!((at(3 * 2 + 1), at(4 * 2 + 1)), (-0.25, -0.5));
    assert_eq!(at(3 * 2), 0.0, "the other channel is untouched");

    // A freed take is not drawn as an empty one: the row goes even and the
    // mapping stops resolving, which is what the generation is for.
    server.retire_buffer(4);
    assert!(!material.holds(4));
    assert!(material.map(4).is_none());

    let _ = std::fs::remove_file(&region_path);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_write_past_the_end_writes_nothing_rather_than_the_next_channel() {
    let path = scratch("clamp");
    let _ = std::fs::remove_file(&path);
    let server = Segment::create(&path).expect("segment");
    let generation = server.publish_buffer(0, 4, 2, 48_000.0).unwrap();
    let region_path = Region::path_for(&path, 0, generation);
    let region = Region::create(&region_path, 8).unwrap();

    let material = SharedMaterial::new(
        Arc::new(SharedSegment::open(&path).expect("segment")),
        path.clone(),
    );
    let take = material.map(0).unwrap();
    take.write_channel(0, 3, &[1.0, 1.0, 1.0]); // one frame fits, two do not
    take.write_channel(7, 0, &[1.0]); // no such channel

    let cells = region.cells();
    let at = |cell: usize| f32::from_bits(cells[cell].load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(at(3 * 2), 1.0);
    assert!(
        (0..8).filter(|&c| at(c) != 0.0).count() == 1,
        "a stroke past the end is dropped, never wrapped into what follows"
    );

    let _ = std::fs::remove_file(&region_path);
    let _ = std::fs::remove_file(&path);
}
