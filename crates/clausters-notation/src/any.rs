//! **An engraver of any binding, as one type.**
//!
//! [`super::Score`] is generic over its [`Engraver`] because each binding
//! reaches the engraver its own way -- libverovio over its C wrapper natively,
//! a JS object in a page -- and a score built by one binding never meets the
//! other's. An **application** is the exception: it is written once and holds
//! whichever score its caller opened, so it needs one type for all of them.
//! [`AnyEngraver`] is that type: any engraver behind a box, with its lock guard
//! boxed too, so `Score<AnyEngraver>` is the one score an application names.

use std::any::Any;

use super::Engraver;

/// The engraver's calls with the guard type erased -- what the box holds.
trait Erased {
    fn lock(&self) -> Box<dyn Any>;
    fn load_data(&self, data: &str) -> bool;
    fn render_svg(&self, page: i32) -> String;
    fn mei(&self) -> String;
    fn edit(&self, action: &str) -> bool;
    fn timemap(&self, options: &str) -> String;
    fn midi_values(&self, xml_id: &str) -> Option<String>;
    fn set_options(&self, options: &str) -> bool;
    fn page_count(&self) -> i32;
}

impl<E> Erased for E
where
    E: Engraver,
    E::Guard: 'static,
{
    fn lock(&self) -> Box<dyn Any> {
        Box::new(Engraver::lock(self))
    }
    fn load_data(&self, data: &str) -> bool {
        Engraver::load_data(self, data)
    }
    fn render_svg(&self, page: i32) -> String {
        Engraver::render_svg(self, page)
    }
    fn mei(&self) -> String {
        Engraver::mei(self)
    }
    fn edit(&self, action: &str) -> bool {
        Engraver::edit(self, action)
    }
    fn timemap(&self, options: &str) -> String {
        Engraver::timemap(self, options)
    }
    fn midi_values(&self, xml_id: &str) -> Option<String> {
        Engraver::midi_values(self, xml_id)
    }
    fn set_options(&self, options: &str) -> bool {
        Engraver::set_options(self, options)
    }
    fn page_count(&self) -> i32 {
        Engraver::page_count(self)
    }
}

/// **Any engraver**, behind a box: what an application's score is opened on.
///
/// It is `Send` because an application is held behind a lock a binding may
/// reach from more than one thread; an engraver that cannot be sent (a page's,
/// which lives on its one thread) says so where it is wrapped.
pub struct AnyEngraver(Box<dyn Erased + Send>);

impl AnyEngraver {
    /// `engraver`, as the one type.
    pub fn new<E>(engraver: E) -> Self
    where
        E: Engraver + Send + 'static,
        E::Guard: 'static,
    {
        Self(Box::new(engraver))
    }
}

impl Engraver for AnyEngraver {
    /// The engraver's own guard, boxed: dropping it releases what it held.
    type Guard = Box<dyn Any>;

    fn lock(&self) -> Self::Guard {
        self.0.lock()
    }
    fn load_data(&self, data: &str) -> bool {
        self.0.load_data(data)
    }
    fn render_svg(&self, page: i32) -> String {
        self.0.render_svg(page)
    }
    fn mei(&self) -> String {
        self.0.mei()
    }
    fn edit(&self, action: &str) -> bool {
        self.0.edit(action)
    }
    fn timemap(&self, options: &str) -> String {
        self.0.timemap(options)
    }
    fn midi_values(&self, xml_id: &str) -> Option<String> {
        self.0.midi_values(xml_id)
    }
    fn set_options(&self, options: &str) -> bool {
        self.0.set_options(options)
    }
    fn page_count(&self) -> i32 {
        self.0.page_count()
    }
}

impl std::fmt::Debug for AnyEngraver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AnyEngraver")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// An engraver holding a document and nothing else, and a lock whose guard
    /// is a real one -- so the box is shown to carry a guard that borrows.
    struct Plain {
        mei: Mutex<String>,
    }

    static LOCK: Mutex<()> = Mutex::new(());

    impl Engraver for Plain {
        type Guard = std::sync::MutexGuard<'static, ()>;
        fn lock(&self) -> Self::Guard {
            LOCK.lock().unwrap_or_else(|e| e.into_inner())
        }
        fn load_data(&self, data: &str) -> bool {
            *self.mei.lock().unwrap() = data.to_string();
            !data.is_empty()
        }
        fn render_svg(&self, _page: i32) -> String {
            String::new()
        }
        fn mei(&self) -> String {
            self.mei.lock().unwrap().clone()
        }
        fn edit(&self, _action: &str) -> bool {
            true
        }
        fn timemap(&self, _options: &str) -> String {
            "[]".into()
        }
        fn midi_values(&self, _xml_id: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_boxed_engraver_answers_as_the_one_inside() {
        let any = AnyEngraver::new(Plain {
            mei: Mutex::new(String::new()),
        });
        {
            let _guard = Engraver::lock(&any);
            // the guard is the engraver's own: its lock is held while it lives
            assert!(LOCK.try_lock().is_err());
        }
        assert!(LOCK.try_lock().is_ok(), "and released with the box");
        assert!(Engraver::load_data(&any, "<mei/>"));
        assert_eq!(Engraver::mei(&any), "<mei/>");
        assert_eq!(Engraver::timemap(&any, "{}"), "[]");
    }
}
