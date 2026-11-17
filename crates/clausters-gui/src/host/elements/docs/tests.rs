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
    let g = v.geometry(RECT, 1.0, &m);
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
    let g = v.geometry(RECT, 1.0, &m);
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
    let g = v.geometry(RECT, 1.0, &m);
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
