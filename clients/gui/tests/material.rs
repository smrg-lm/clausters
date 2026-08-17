//! The host's mirror of the segment, contrasted against the server's own.
//!
//! `host::shm` and `host::material` read a `#[repr(C)]` layout the server
//! crate defines, by hand, because this crate must not depend on that one. So
//! the thing worth testing is exactly that: that the two agree. This suite is
//! the one place they can be put side by side, and it is gated on
//! `standalone` — the feature that links the server — for the same reason.
//!
//! It is what a version number alone cannot do. Following the ABI counter kept
//! this reader compiling while its **size check** rejected every real segment
//! of that version, which is a mirror that agrees on the number and not on the
//! layout.

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
