//! The documentation panel's tests: the books and documents it is given,
//! the pages it goes to, finding, and help on a name.

use super::*;
use crate::host::clipboard::Clip;
use crate::host::widget::element::Mods;

fn panel() -> DocsView {
    let books = serde_json::json!([{
        "prefix": "",
        "title": "The Book",
        "parts": [{"title": null, "chapters": [
            {"title": "Guide", "path": "guide.md", "children": [
                {"title": "Buffers", "path": "buffers.md", "children": []}
            ]},
            {"title": "Reference", "path": "ref.md", "children": []}
        ]}]
    }]);
    let documents = serde_json::json!({
        "guide.md": "# Guide\n\nRead [the buffers](buffers.md#reading) first.\n\nOut: [site](https://example.org).\n",
        "buffers.md": "# Buffers\n\nA buffer holds samples.\n\n## Reading\n\nA buffer is read.\n\n```python\nb = Buffer.read(path)\n```\n",
        "ref.md": "# Reference\n\n## `Buffer.read`\n\nReads a file into a buffer.\n",
    });
    let mut props = Map::new();
    props.insert("books".into(), Value::from(books.to_string()));
    props.insert("documents".into(), documents);
    DocsView::new(&props)
}

fn page(v: &DocsView) -> String {
    v.info()
        .into_iter()
        .find(|(k, _)| k == "page")
        .and_then(|(_, v)| v.as_str().map(str::to_string))
        .unwrap()
}

const RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 600.0,
    h: 400.0,
};

fn input(m: &Metrics) -> Input<'_> {
    Input {
        metrics: m,
        indent: 0.0,
        rect: RECT,
        scale: 1.0,
        mods: Mods::default(),
        viewport: (600.0, 400.0),
        clicks: 1,
        time: None,
        header: None,
    }
}

fn keys(v: &mut DocsView, clip: &mut Clip, mods: Mods, keys: &[Key]) {
    for k in keys {
        let mut input = KeyInput {
            mods,
            clipboard: &mut *clip,
            cursor: None,
        };
        v.key(k, &mut input);
    }
}

/// The press that lands on the link `href` of the page on screen.
fn on_link(v: &DocsView, href: &str) -> (f64, f64) {
    let m = Metrics::default();
    let g = v.geometry(RECT, 1.0, &m, None);
    let (scroll, laid) = v.settle(&g);
    let (r, _) = laid
        .links
        .iter()
        .find(|(_, h)| h.ends_with(href))
        .expect("the link");
    (
        f64::from(g.text.x + r.x + 1.0),
        f64::from(g.text.y + r.y - scroll + r.h * 0.5),
    )
}

/// **The start page lists the books and their chapters; a chapter opens
/// its document, a link in it another at its heading, back and forward
/// walk them**, and a link out of the documentation is reported.
#[test]
fn pages_are_gone_to_and_back() {
    let m = Metrics::default();
    let mut v = panel();
    assert_eq!(page(&v), "home");
    let at = on_link(&v, "doc:guide.md");
    v.press(at, &input(&m));
    assert_eq!(page(&v), "doc:guide.md");
    let at = on_link(&v, "buffers.md#reading");
    v.press(at, &input(&m));
    assert_eq!(page(&v), "doc:buffers.md#reading");
    let g = v.geometry(RECT, 1.0, &m, None);
    let (scroll, laid) = v.settle(&g);
    // toward the heading, as far as a short page scrolls
    let most = (laid.height - g.text.h * 0.5).max(0.0);
    assert_eq!(
        scroll,
        laid.anchor("reading").unwrap().min(most),
        "at the heading"
    );
    assert!(scroll > 0.0);
    assert!(v.step(-1));
    assert_eq!(page(&v), "doc:guide.md");
    assert!(v.step(1));
    assert_eq!(page(&v), "doc:buffers.md#reading");
    v.step(-1);
    let at = on_link(&v, "https://example.org");
    let Claim::Take(t) = v.press(at, &input(&m)) else {
        panic!("taken")
    };
    let args = t.events.into_messages();
    assert_eq!(
        args[0],
        vec![
            OscType::String("link".into()),
            OscType::String("https://example.org".into())
        ]
    );
    assert_eq!(page(&v), "doc:guide.md", "the panel stays");
}

/// **Ctrl+F finds in the page and Ctrl+Shift+F in every document**: the
/// page's matches counted and stepped through, the search's results listed
/// and one opened, its word found there.
#[test]
fn finding_in_the_page_and_everywhere() {
    let mut v = panel();
    let mut clip = Clip::default();
    v.set("open", &Value::from("buffers.md"));
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    keys(&mut v, &mut clip, ctrl, &[Key::Char('f')]);
    keys(
        &mut v,
        &mut clip,
        Mods::default(),
        &"buffer".chars().map(Key::Char).collect::<Vec<_>>(),
    );
    let m = Metrics::default();
    let mut mesh = clausters_gui_draw::paint::Mesh::new();
    let theme = Theme::default();
    let world = crate::host::world::World::default();
    let ctx = Ctx {
        world: &world,
        metrics: &m,
        rect: RECT,
        indent: 0.0,
        clip: None,
        scale: 1.0,
        time: None,
        focused: true,
        hovered: false,
        clock: 0.0,
        text_fit: 1.0,
        header: None,
    };
    v.draw(&mut Draw::new(&mut mesh, &m, &theme), &ctx);
    assert!(v.match_count() >= 3, "{} in the page", v.match_count());
    assert!(mesh.colors_by_y().any(|(_, c)| c == theme.find_current));
    keys(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    assert_eq!(v.current, 1);
    keys(
        &mut v,
        &mut clip,
        Mods {
            shift: true,
            ..ctrl
        },
        &[Key::Char('F')],
    );
    assert_eq!(page(&v), "search:buffer");
    let Page::Search { hits, .. } = v.page() else {
        panic!()
    };
    assert!(hits.len() >= 2);
    let first = hits[0].clone();
    let target = format!(
        "{}#{}",
        first.path,
        first.heading.as_deref().map(slugify).unwrap_or_default()
    );
    let at = on_link(&v, &target);
    v.press(at, &input(&m));
    assert!(page(&v).starts_with(&format!("doc:{}", first.path)));
    assert_eq!(v.scope, Scope::Page, "its word found in the page");
}

/// **Help on a name opens the entry that is it**, else what the owner sent
/// with it.
#[test]
fn help_opens_the_entry_or_the_owners_text() {
    let mut v = panel();
    v.set(
        "help",
        &Value::from(r#"{"name": "read", "qualname": "Buffer.read"}"#),
    );
    assert_eq!(page(&v), "doc:ref.md#bufferread");
    v.set(
        "help",
        &Value::from(r#"{"name": "zzz", "text": "Help on zzz.", "source": "pydoc"}"#),
    );
    assert_eq!(page(&v), "text:zzz");
}

/// **A code block's copy button copies its code**, through the window's
/// copy.
#[test]
fn a_code_block_is_copied() {
    let m = Metrics::default();
    let mut v = panel();
    v.set("open", &Value::from("buffers.md"));
    let g = v.geometry(RECT, 1.0, &m, None);
    let (scroll, laid) = v.settle(&g);
    let (r, code) = laid.copies[0].clone();
    let at = (
        f64::from(g.text.x + r.x + 1.0),
        f64::from(g.text.y + r.y - scroll + 1.0),
    );
    let Claim::Take(mut t) = v.press(at, &input(&m)) else {
        panic!()
    };
    assert_eq!(t.events.take_command().as_deref(), Some("copy"));
    let mut clip = Clip::default();
    let mut ki = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clip,
        cursor: None,
    };
    v.verb(Verb::Copy, &mut ki);
    assert_eq!(clip.text(), code);
}

/// **A picture a page names is read and drawn at its size**: laid out, the
/// page writes its description and asks for the file under the root its
/// document was read from; read, it lays the picture out at its own size,
/// narrowed to the page; a file that is not a picture keeps its description.
#[test]
fn a_picture_a_page_names_is_read_and_laid_out() {
    let mut props = Map::new();
    props.insert(
        "documents".into(),
        serde_json::json!({
            "book/ch/a.md": "# A\n\n![a dot](../img/dot.png)\n\n![wide](../img/wide.png)\n\n![none](../img/none.png)\n",
        }),
    );
    props.insert(
        "roots".into(),
        serde_json::json!({"book/": "/books/b", "": "/x"}),
    );
    let mut v = DocsView::new(&props);
    v.set("open", &Value::from("book/ch/a.md"));
    let laid = v.laid(300.0, 2.0);
    let pictures = |laid: &Laid| {
        laid.items
            .iter()
            .filter_map(|i| match i {
                Item::Picture { rect, url } => Some((url.clone(), rect.w, rect.h)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(pictures(&laid).is_empty(), "nothing read yet");
    let mut asks = v.wants_files();
    asks.sort_by(|a, b| a.path.cmp(&b.path));
    let paths: Vec<&str> = asks.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/books/b/img/dot.png",
            "/books/b/img/none.png",
            "/books/b/img/wide.png"
        ]
    );
    assert!(v.wants_files().is_empty(), "asked once");
    let png =
        |w: u32, h: u32| crate::host::png::encode_rgba(w, h, &vec![200; (w * h * 4) as usize]);
    assert!(v.read(asks[0].ticket, Ok(png(10, 6))));
    assert!(v.read(asks[1].ticket, Ok(b"not a picture".to_vec())));
    assert!(v.read(asks[2].ticket, Ok(png(600, 100))));
    assert!(!v.read(asks[2].ticket, Ok(png(1, 1))), "read once");
    let laid = v.laid(300.0, 2.0);
    let shown = pictures(&laid);
    // at the page's scale (its size over the panel's own text size), and
    // narrowed to the page in proportion
    let scale = 2.0 / v.text_size;
    assert_eq!(
        shown[0],
        ("../img/dot.png".into(), 10.0 * scale, 6.0 * scale)
    );
    assert_eq!(shown[1].0, "../img/wide.png");
    assert!((shown[1].1 - 300.0).abs() < 1e-3 && (shown[1].2 - 50.0).abs() < 1e-3);
    assert_eq!(
        shown.len(),
        2,
        "the file that is not a picture keeps its description"
    );
    assert!(
        v.picture_uv("../img/dot.png").is_some(),
        "packed into the sheet"
    );
}

/// **A code block wider than the page scrolls across on its own**: Shift
/// with the wheel over it moves its lines and no further than its longest
/// one, the page stays where it was, and a block that fits does not move.
#[test]
fn a_long_code_line_scrolls_across_its_block() {
    let long = "x = ".to_string() + &"1 + ".repeat(80) + "1";
    let mut props = Map::new();
    props.insert(
        "documents".into(),
        serde_json::json!({
            "a.md": format!("# A\n\n```python\n{long}\n```\n\n```python\nshort = 1\n```\n"),
        }),
    );
    let mut v = DocsView::new(&props);
    v.set("open", &Value::from("a.md"));
    let m = Metrics::default();
    let mut input = input(&m);
    let g = v.geometry(RECT, 1.0, &m, None);
    let (scroll, laid) = v.settle(&g);
    assert_eq!(laid.pans.len(), 2);
    let (wide, fits) = (laid.pans[0], laid.pans[1]);
    assert!(wide.most() > 0.0 && fits.most() == 0.0, "{wide:?} {fits:?}");
    let (left, top) = (g.text.x, g.text.y - scroll);
    let over = |b: &crate::host::graphics::docs::layout::Pan| {
        (
            f64::from(left + b.rect.x + 10.0),
            f64::from(top + b.rect.y + b.rect.h * 0.5),
        )
    };
    input.mods.shift = true;
    assert!(v.wheel(over(&wide), (0.0, -1.0), &input).is_some());
    let moved = v.pan_of(0);
    assert!(moved > 0.0, "it moved across");
    assert_eq!(v.scroll.get(), scroll, "the page stayed");
    for _ in 0..500 {
        v.wheel(over(&wide), (0.0, -1.0), &input);
    }
    assert!(
        (v.pan_of(0) - wide.most()).abs() < 1e-3,
        "no further than its line"
    );
    assert!(v.wheel(over(&fits), (0.0, -1.0), &input).is_none() || v.pan_of(1) == 0.0);
    assert_eq!(v.pan_of(1), 0.0, "a block that fits does not move");
    // a sideways wheel, without Shift, moves it back
    input.mods.shift = false;
    v.wheel(over(&wide), (1.0, 0.0), &input);
    assert!(v.pan_of(0) < wide.most());
}
