//! **Which window's menu the dock's bar shows**, decided from the dock:
//! the host's side of [`crate::host::menubar`], kept with the dock because
//! it reads the dock's panes, its edges and its focus.

use super::{Dock, Edge};
use crate::host::Host;
use crate::host::gestures::GestureEffect;
use crate::host::menu::Entry;
use crate::host::menubar::{MenuShown, entries_of, own_entries, shown_from};
use crate::host::widget::WidgetKind;

impl Host {
    /// **Which window's menu the dock's bar shows**: the dock's application
    /// bar (a window along its top edge with a menu) shows the menu of the
    /// application on the page shown -- a panel's window with a menu, the
    /// one that last had the keyboard -- and that window draws no bar of its
    /// own; with none, the bar shows its own. Answers what the front carries
    /// out: each window whose bar changed drawn again, and **the bar's owner
    /// told whose menu it shows** (`"menu_from" <window>`, its own id when
    /// its own) each time that changes -- so an owner whose bar holds tools
    /// of its own menu shows them only beside it.
    pub fn show_menus(&mut self, dock: &Dock) -> Vec<GestureEffect> {
        let has_menu = |host: &Host, id: i32| host.window_def(id).and_then(own_entries).is_some();
        let shown: Vec<i32> = dock.placed().map(|(id, _)| id).collect();
        let bar = shown
            .iter()
            .copied()
            .find(|&id| dock.edge_of(id) == Some(Edge::Top) && has_menu(self, id));
        let apps: Vec<i32> = shown
            .iter()
            .copied()
            .filter(|&id| dock.edge_of(id).is_none() && has_menu(self, id))
            .collect();
        let source = bar.and_then(|_| {
            dock.focus()
                .filter(|f| apps.contains(f))
                .or(self.menu_chosen.filter(|c| apps.contains(c)))
                .or_else(|| apps.first().copied())
        });
        if source.is_some() {
            self.menu_chosen = source;
        }
        let mut changed = Vec::new();
        let mut told = Vec::new();
        for id in dock.panes() {
            let was = self.window_def(id).and_then(shown_from);
            let want = match (bar, source) {
                (Some(b), Some(src)) if id == b => {
                    // copied only when it is not the one shown already: this
                    // runs with every motion of the pointer
                    let entries = self.window_def(src).and_then(own_entries).unwrap_or(&[]);
                    let shows = self.window_def(id).is_some_and(|tree| {
                        shown_from(tree) == Some(src) && entries_of(tree) == Some(entries)
                    });
                    if shows {
                        continue;
                    }
                    MenuShown::From {
                        window: src,
                        entries: entries.to_vec(),
                    }
                }
                (Some(_), Some(src)) if id == src => MenuShown::Away,
                _ => MenuShown::Own,
            };
            let from = match &want {
                MenuShown::From { window, .. } => Some(*window),
                _ => None,
            };
            if self.set_menu_shown(id, want) {
                changed.push(GestureEffect::Redraw(id));
                if from != was {
                    let seq = self.outbox.borrow_mut().stamp(id, id);
                    told.push(GestureEffect::Emit {
                        def_id: id,
                        widget_id: id,
                        seq,
                        args: vec![
                            clausters_core::osc::OscType::String("menu_from".into()),
                            clausters_core::osc::OscType::Int(from.unwrap_or(id)),
                        ],
                    });
                }
            }
        }
        changed.extend(told);
        changed
    }

    /// Window `id`'s bar shows `want`; answers whether that changed it.
    pub(crate) fn set_menu_shown(&mut self, id: i32, want: MenuShown) -> bool {
        let Some(tree) = self.window_def_mut(id) else {
            return false;
        };
        let WidgetKind::Window { menu_shown, .. } = &mut tree.kind else {
            return false;
        };
        if *menu_shown == want {
            return false;
        }
        *menu_shown = want;
        true
    }

    /// **Window `id`'s own menu may have changed**: a bar showing it in its
    /// place shows it as it is now. Answers that bar when what it shows
    /// changed, to be drawn again.
    pub(crate) fn menu_changed(&mut self, id: i32) -> Option<i32> {
        let entries = self
            .window_def(id)
            .and_then(own_entries)
            .map(<[Entry]>::to_vec)
            .unwrap_or_default();
        let bar = self
            .window_defs
            .iter()
            .find(|(_, tree)| shown_from(tree) == Some(id))
            .map(|(&w, _)| w)?;
        self.set_menu_shown(
            bar,
            MenuShown::From {
                window: id,
                entries,
            },
        )
        .then_some(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::menubar;
    use crate::host::{ClientId, GUI_DEF};
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    fn def(host: &mut Host, id: i32, json: &str) {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(id), OscType::String(json.into())],
            }),
            ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
    }

    /// **The dock decides which menu its bar shows**: the application's on
    /// the page shown, which then draws no bar of its own; settled, nothing
    /// changes again; and with no application shown the bar shows its own.
    #[test]
    fn the_bar_shows_the_application_on_the_page_shown() {
        let mut host = Host::new();
        def(
            &mut host,
            1,
            r#"{"type":"window","menu":[{"label":"File","menu":[{"label":"Open","verb":"open"}]}],
                "children":[]}"#,
        );
        def(
            &mut host,
            2,
            r#"{"type":"window","dock_edge":"top","h":40,
                "menu":[{"label":"Code","menu":[{"label":"New","verb":"new_file"}]}],
                "children":[]}"#,
        );
        let mut dock = Dock::new((600, 440), host.metrics.resolved(1.0));
        dock.add_edge(2, Edge::Top, 40);
        dock.add(1);
        let effects = host.show_menus(&dock);
        let mut changed: Vec<i32> = effects
            .iter()
            .filter_map(|e| match e {
                GestureEffect::Redraw(id) => Some(*id),
                _ => None,
            })
            .collect();
        changed.sort();
        assert_eq!(changed, [1, 2]);
        let told = |effects: &[GestureEffect]| -> Vec<(i32, Vec<OscType>)> {
            effects
                .iter()
                .filter_map(|e| match e {
                    GestureEffect::Emit { def_id, args, .. } => Some((*def_id, args.clone())),
                    _ => None,
                })
                .collect()
        };
        let from = |id| vec![OscType::String("menu_from".into()), OscType::Int(id)];
        assert_eq!(told(&effects), [(2, from(1))], "the bar's owner is told");
        let shown =
            |host: &Host, id| menubar::entries(host.window_def(id).unwrap()).map(<[_]>::to_vec);
        let own = |host: &Host, id| own_entries(host.window_def(id).unwrap()).map(<[_]>::to_vec);
        assert_eq!(
            shown(&host, 2),
            own(&host, 1),
            "the bar shows the application's"
        );
        assert_eq!(shown(&host, 1), None, "which shows none of its own");
        assert!(
            host.show_menus(&dock).is_empty(),
            "settled: nothing changes again"
        );
        dock.remove(1);
        let effects = host.show_menus(&dock);
        assert_eq!(shown(&host, 2).unwrap()[0].label, "Code");
        assert_eq!(told(&effects), [(2, from(2))], "its own again");
    }

    /// **A verb of the bar's own menu is its own, while it shows another
    /// window's**: a tool of the bar names an entry of the bar's menu, and
    /// the same place in the menu it shows is another verb of another
    /// window. Found by the path in one menu and read in the other, the
    /// code editor's Interrupt performed an application's display settings.
    #[test]
    fn a_verb_of_the_bar_s_own_menu_stays_its_own() {
        use crate::host::gestures::{GestureCtx, Gestures};
        let mut host = Host::new();
        def(
            &mut host,
            1,
            r#"{"type":"window","menu":[{"label":"File","menu":[{"label":"Open","verb":"open"}]}],
                "children":[]}"#,
        );
        def(
            &mut host,
            2,
            r#"{"type":"window","dock_edge":"top","h":40,
                "menu":[{"label":"Code","menu":[{"label":"New","verb":"new_file"}]}],
                "children":[]}"#,
        );
        let mut dock = Dock::new((600, 440), host.metrics.resolved(1.0));
        dock.add_edge(2, Edge::Top, 40);
        dock.add(1);
        host.show_menus(&dock);
        let ctx = GestureCtx::new(2, 600, 40);
        let told: Vec<(i32, Vec<OscType>)> = Gestures::default()
            .command(&mut host, &ctx, "new_file")
            .into_iter()
            .filter_map(|e| match e {
                GestureEffect::Emit { def_id, args, .. } => Some((def_id, args)),
                _ => None,
            })
            .collect();
        assert_eq!(
            told,
            [(
                2,
                vec![
                    OscType::String("menu".into()),
                    OscType::String("new_file".into())
                ]
            )]
        );
    }

    /// **A window redefined keeps what the dock decided of its bar**: an
    /// application that rebuilds its window (a panel shown or hidden) draws
    /// no bar of its own in the frame before the dock decides again, which
    /// showed the menu twice for an instant; and the dock's bar shows the
    /// new menu at once.
    #[test]
    fn a_window_redefined_keeps_its_bar_away() {
        let mut host = Host::new();
        let app = |label: &str| {
            format!(
                r#"{{"type":"window","menu":[{{"label":"{label}","menu":[{{"label":"Open","verb":"open"}}]}}],
                    "children":[]}}"#
            )
        };
        def(&mut host, 1, &app("File"));
        def(
            &mut host,
            2,
            r#"{"type":"window","dock_edge":"top","h":40,
                "menu":[{"label":"Code","menu":[{"label":"New","verb":"new_file"}]}],
                "children":[]}"#,
        );
        let mut dock = Dock::new((600, 440), host.metrics.resolved(1.0));
        dock.add_edge(2, Edge::Top, 40);
        dock.add(1);
        host.show_menus(&dock);
        def(&mut host, 1, &app("Edit"));
        let shown =
            |host: &Host, id| menubar::entries(host.window_def(id).unwrap()).map(<[_]>::to_vec);
        assert_eq!(shown(&host, 1), None, "the rebuilt window draws no bar");
        assert_eq!(
            shown(&host, 2).unwrap()[0].label,
            "Edit",
            "the dock's bar shows the new menu"
        );
        assert!(
            host.show_menus(&dock).is_empty(),
            "nothing left for the dock to decide"
        );
    }
}
