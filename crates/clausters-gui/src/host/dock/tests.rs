//! The dock's tests: the arrangement and the hand with no device, and
//! the composed picture against each pane's own.

use super::*;

/// **Panes are equal columns with a divider between them**, the last taking
/// what the division left; a pane taken out gives its room to the others.
#[test]
fn panes_are_equal_columns_and_share_a_removed_panes_room() {
    let mut ws = Dock::new((403, 100), 3);
    ws.add(1);
    assert_eq!(
        ws.rect(1),
        Some(PaneRect {
            x: 0,
            y: 0,
            w: 403,
            h: 100
        })
    );
    ws.add(2);
    ws.add(3);
    let rects: Vec<_> = ws.placed().map(|(_, r)| (r.x, r.w)).collect();
    assert_eq!(rects, [(0, 132), (135, 132), (270, 133)]);
    assert!(ws.remove(2));
    let rects: Vec<_> = ws.placed().map(|(id, r)| (id, r.x, r.w)).collect();
    assert_eq!(rects, [(1, 0, 200), (3, 203, 200)]);
    assert!(!ws.remove(2), "a pane is taken out once");
    assert!(!ws.add(1), "a pane is added once");
}

/// **A point is in the pane under it, in the pane's own pixels**; a divider
/// is in none.
#[test]
fn a_point_is_read_in_the_pane_under_it() {
    let mut ws = Dock::new((204, 50), 4);
    ws.add(7);
    ws.add(8);
    assert_eq!(ws.at(10.0, 5.0), Some((7, (10.0, 5.0))));
    assert_eq!(ws.at(110.0, 20.0), Some((8, (6.0, 20.0))));
    assert_eq!(ws.at(101.0, 20.0), None, "the divider");
    assert_eq!(ws.at(10.0, 60.0), None, "below the surface");
}

/// **A press holds its pane until the release**: motion keeps going to it in
/// its own pixels wherever the pointer is, no pane hears the pointer leave
/// meanwhile, and the keyboard follows the press.
#[test]
fn a_press_holds_its_pane_until_the_release() {
    let mut ws = Dock::new((204, 50), 4);
    ws.add(7);
    ws.add(8);
    assert_eq!(ws.focus(), Some(8), "the pane added last has the keyboard");
    let m = ws.moved(10.0, 10.0);
    assert_eq!(m.to, Some((7, (10.0, 10.0))));
    assert_eq!(m.left, None);
    assert_eq!(ws.pressed(), Some(7));
    assert_eq!(ws.focus(), Some(7), "a press gives its pane the keyboard");
    let m = ws.moved(150.0, 10.0);
    assert_eq!(m.to, Some((7, (150.0, 10.0))), "the drag stays in pane 7");
    assert_eq!(m.left, None);
    assert_eq!(
        ws.left(),
        None,
        "a held pane keeps the pointer off the surface"
    );
    assert_eq!(ws.released(), Some(7));
    let m = ws.moved(150.0, 10.0);
    assert_eq!(m.to, Some((8, (46.0, 10.0))));
    assert_eq!(m.left, Some(7), "crossing into pane 8 leaves pane 7");
    assert_eq!(ws.left(), Some(8));
    assert_eq!(ws.pointer(), None);
}

/// **A pane taken out lets go of the hand** that was on it, and the keyboard
/// goes to the pane left last.
#[test]
fn a_removed_pane_lets_go_of_the_hand() {
    let mut ws = Dock::new((204, 50), 4);
    ws.add(7);
    ws.add(8);
    ws.moved(150.0, 10.0);
    ws.pressed();
    ws.remove(8);
    assert_eq!(ws.held(), None);
    assert_eq!(ws.pointer(), None);
    assert_eq!(ws.focus(), Some(7));
}

/// **The composed picture is each pane's own picture, in its place**: two
/// windows drawn as panes of one dock read back, region by region, as
/// each window's own capture at the same size does -- within the two levels
/// a channel two draws of one picture may differ by -- and the divider
/// between them is the clear color.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn the_composed_picture_is_each_panes_own_picture() {
    use std::collections::HashMap;

    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use crate::gpu::Gpu;
    use crate::host::frame::{self, Batches, FrameInputs, clear_color};
    use crate::host::world::World;
    use crate::view::Renderers;

    let (w, h, gap) = (200u32, 150u32, 4u32);
    let Ok(dock) = pollster::block_on(Gpu::offscreen(2 * w + gap, h, 4)) else {
        eprintln!("no adapter on this machine: nothing to draw offscreen with");
        return;
    };
    let mut host = crate::host::Host::new();
    let from = crate::host::ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )));
    let defs = [
        (
            1,
            r#"{"type":"window","w":200,"h":150,"children":[
                {"id":2,"type":"slider","value":0.7}]}"#,
        ),
        (
            3,
            r#"{"type":"window","w":200,"h":150,"children":[
                {"id":4,"type":"button","text":"play"},
                {"id":5,"type":"knob","value":0.25}]}"#,
        ),
    ];
    for (id, json) in defs {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(id), OscType::String(json.into())],
            }),
            from,
        );
    }
    let mut ws = Dock::new((2 * w + gap, h), gap);
    let mut panes = Vec::new();
    let mut own = Vec::new();
    for (id, _) in defs {
        ws.add(id);
    }
    for (id, _) in defs {
        let rect = ws.rect(id).expect("placed");
        assert_eq!((rect.w, rect.h), (w, h));
        let mut gpu = Gpu::pane(&dock, rect.w, rect.h);
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let inputs = FrameInputs {
            metrics: host.metrics_for(id),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let tree = host.window_def(id).expect("defined");
        frame::render(
            &mut gpu,
            &mut renderers,
            &mut batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            tree,
            &inputs,
            &host.theme,
            true,
        );
        // the window's own picture, at the pane's size, by the capture path
        let mut alone = Gpu::pane(&dock, rect.w, rect.h);
        let mut alone_renderers = Renderers::new(&alone.device, alone.target());
        let mut alone_batches = Batches::new(&alone.device, alone.target());
        let readback = frame::capture(
            &mut alone,
            &mut alone_renderers,
            &mut alone_batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            tree,
            &inputs,
            &host.theme,
        )
        .expect("reads back");
        readback.wait(&alone.device).expect("mapped");
        own.push((rect, readback.pixels().2));
        panes.push((rect, gpu));
    }
    let clear = clear_color(&host.theme);
    let views: Vec<_> = panes
        .iter()
        .map(|(rect, gpu)| (*rect, gpu.pane_view().expect("a pane")))
        .collect();
    let mut composer = Composer::new(&dock.device, dock.config.format);
    let readback = composer.capture(&dock, clear, &views).expect("reads back");
    readback.wait(&dock.device).expect("mapped");
    let (cw, ch, composed) = readback.pixels();
    assert_eq!((cw, ch), (2 * w + gap, h));
    for (rect, pixels) in &own {
        let mut differ = 0;
        let mut drawn = 0;
        for y in 0..rect.h {
            for x in 0..rect.w {
                let a = ((y * rect.w + x) * 4) as usize;
                let b = (((rect.y + y) * cw + rect.x + x) * 4) as usize;
                if (0..3).any(|c| (i32::from(pixels[a + c]) - i32::from(composed[b + c])).abs() > 2)
                {
                    differ += 1;
                }
                if (0..3).any(|c| pixels[a + c] != pixels[c]) {
                    drawn += 1;
                }
            }
        }
        assert!(drawn > 300, "the window draws something: {drawn} pixels");
        assert_eq!(differ, 0, "pane at x={} is its own picture", rect.x);
    }
    let bg = [clear.r, clear.g, clear.b].map(|c| (c * 255.0).round() as i32);
    let x = w + gap / 2;
    for y in [0, h / 2, h - 1] {
        let at = ((y * cw + x) * 4) as usize;
        let px: Vec<i32> = composed[at..at + 3].iter().map(|&c| i32::from(c)).collect();
        assert!(
            px.iter().zip(bg).all(|(a, b)| (a - b).abs() <= 2),
            "the divider is the clear color at y={y}: {px:?} against {bg:?}"
        );
    }
}
